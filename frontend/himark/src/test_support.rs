use imba::store::Store;

use crate::app::pane_width;
use crate::{workbench_geometry, Application, EditorIdView, Panel};

struct TestUris;

impl crate::higent::ResourceUriMap for TestUris {
    fn uri_of(&self, location: &crate::ResourceLocation) -> crate::higent::ResourceUri {
        crate::higent::ResourceUri::new(format!("file:///{}", location.path().join("/")))
    }

    fn location_of(
        &self,
        uri: &crate::higent::ResourceUri,
        kind: crate::ResourceType,
        _authority: &crate::Authority,
    ) -> Option<crate::ResourceLocation> {
        let path: Vec<String> = uri
            .as_str()
            .strip_prefix("file://")?
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();

        Some(crate::ResourceLocation::new(
            kind,
            crate::Authority::new("test"),
            path,
        ))
    }
}

#[derive(Clone, Default)]
struct SeededSessions(u64);

pub fn seed_session_folders(
    store: &mut Store,
    folders: &[crate::ResourceLocation],
) -> crate::SessionId {
    let host = crate::SessionId::local_default(store).host;
    crate::higent::Agents::seed(store, host, "Test Host");
    crate::higent::Hosts::install_uris(store, host, std::sync::Arc::new(TestUris));
    let mut minted = 0;
    store.update::<SeededSessions>(|seeded| {
        seeded.0 += 1;
        minted = seeded.0;
    });
    let id = crate::SessionId {
        host,
        session: format!("test-session:/{minted}"),
    };
    let uris = crate::higent::Hosts::uris(store, host).expect("installed above");
    let working_directories: Vec<String> = folders
        .iter()
        .map(|folder| uris.uri_of(folder).into_string())
        .collect();
    crate::higent::Agents::set_channel(
        store,
        &id,
        crate::higent::SessionChannel {
            provider: "test".to_owned(),
            chats: rpds::VectorSync::new_sync(),
            default_chat: None,
            working_directories: working_directories.iter().cloned().collect(),
            config: None,
        },
    );

    crate::higent::Agents::add_sessions(
        store,
        host,
        vec![ahp_types::state::SessionSummary {
            provider: "test".to_owned(),
            title: String::new(),
            status: 0,
            activity: None,
            project: None,
            working_directories: Some(working_directories),
            annotations: None,
            resource: id.session.clone(),
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
    id: &crate::SessionId,
    folders: &[crate::ResourceLocation],
) {
    let uris = crate::higent::Hosts::uris(store, id.host).expect("a seeded session");
    let mut channel = crate::higent::Agents::channel(store, id).expect("a seeded session");
    for folder in folders {
        let uri = uris.uri_of(folder).into_string();
        if !channel.working_directories.iter().any(|held| held == &uri) {
            channel.working_directories.push_back_mut(uri);
        }
    }
    crate::higent::Agents::set_channel(store, id, channel);
}

#[allow(dead_code)]
fn pane_height_content(store: &Store, view: EditorIdView) -> Option<f32> {
    Some(crate::OpenDocuments::document_ref(store, view.document())?.content_height(view.editor()))
}

impl Application {
    pub fn sole_window(&self) -> crate::WindowId {
        let windows = self.window_ids();
        assert!(windows.len() <= 1, "multiple windows: tests must name one");
        *windows.first().expect("a window")
    }

    fn workbench(&self) -> &crate::Workbench {
        crate::Windows::window_ref(self.store(), self.sole_window())
            .expect("the window entity")
            .workbench()
    }

    pub fn viewport_size(&self) -> skia_safe::Size {
        crate::Windows::window_ref(self.store(), self.sole_window())
            .expect("the window entity")
            .viewport_size()
    }

    pub fn plugin_modal(&self) -> Option<&dyn crate::ModalView> {
        crate::Windows::window_ref(self.store(), self.sole_window())?.plugin_modal()
    }

    pub fn focused_document_text(&self) -> Option<String> {
        let document = crate::OpenDocuments::document_ref(
            self.store(),
            crate::Windows::window_ref(self.store(), self.sole_window())?.focused_document_id()?,
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
            crate::OpenDocuments::document_ref(self.store(), entity.document()).expect("document");
        let live = document.element_heights(entity.editor());
        let width = document.layout_width(entity.editor());
        let mut fresh = crate::EditorView::complete(
            document.clone(),
            width,
            &::editor::embedded_fonts::source()(),
            &::editor::env::Themes::of(self.store()),
        );
        fresh.reveal_caret(
            document.caret_byte(entity.editor()),
            &::editor::embedded_fonts::source()(),
            &::editor::env::Themes::of(self.store()),
        );
        let text = document.text().byte_string(0, document.text().byte_count());
        (live, fresh.element_heights(), text)
    }

    pub fn focused_document_is_header_at(&self, byte: u32) -> bool {
        let Some(document) = crate::Windows::window_ref(self.store(), self.sole_window())
            .and_then(|window| window.focused_document_id())
            .and_then(|id| crate::OpenDocuments::document_ref(self.store(), id))
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
        let mut offenders = Vec::new();
        let mut index = 0usize;
        let store = self.store();
        let mut check = |index: usize, entity: EditorIdView| {
            {
                if let Some(document) = crate::OpenDocuments::document_ref(store, entity.document())
                {
                    let (pending, ..) = document.probe_state(entity.editor());
                    if pending.is_some() {
                        offenders.push((index, f32::NAN, f32::NAN));
                        return;
                    }
                    let width = document.layout_width(entity.editor());
                    let fonts = ::editor::embedded_fonts::source()();
                    let theme = ::editor::theme::Theme::embedded();
                    let mut reference =
                        crate::EditorView::complete(document.clone(), width, &fonts, &theme);

                    reference.reveal_caret(document.caret_byte(entity.editor()), &fonts, &theme);
                    let actual = document.content_height(entity.editor());
                    let expected = reference.content_height();
                    if (actual - expected).abs() > 0.5 {
                        offenders.push((index, actual, expected));
                    }
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
        let document = crate::OpenDocuments::document_ref(self.store(), entity.document())?;
        let byte_count = document.text().byte_count().min(u32::MAX as usize) as u32;
        let extras = document.extras_keyed(entity.editor());
        let interval = ::editor::OverlaidMarkup::new(document.markup(), &extras)
            .all_inlays_in(0..byte_count)
            .into_iter()
            .find(|interval| {
                matches!(
                    interval.inlay.mode(),
                    crate::InlayMode::Above | crate::InlayMode::Under
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
        let document = crate::OpenDocuments::document_ref(self.store(), entity.document())?;
        Some(document.caret_byte(entity.editor()))
    }

    pub fn focused_reveal_pending(&self) -> Option<bool> {
        let pane = self.workbench().root.focused_pane().editor()?;
        let entity = pane.content();
        let document = crate::OpenDocuments::document_ref(self.store(), entity.document())?;
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
            if let Some(document) = crate::OpenDocuments::document_ref(store, entity.document()) {
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
        let document = crate::OpenDocuments::document_ref(self.store(), view.document())?;
        if document.text().view().byte_count() == 0 {
            return None;
        }
        document
            .markup()
            .block_marks_in(0..8)
            .ids()
            .iter()
            .find_map(|id| match *id {
                ::editor::StyleId::Header(level) => Some(Some(level)),
                _ => None,
            })
            .or(Some(None))
    }

    pub fn document_count(&self) -> usize {
        crate::OpenDocuments::list(self.store()).len()
    }

    pub fn pane_count(&self) -> usize {
        let mut count = 0;
        self.workbench().root.for_each_pane(&mut |_| count += 1);
        count
    }

    pub fn for_each_plugin_panel(&self, visit: &mut dyn FnMut(&dyn crate::DynPanelView)) {
        self.workbench().root.for_each_pane(&mut |panel| {
            if let Panel::Plugin(view) = panel {
                visit(view.as_ref());
            }
        });
    }

    pub fn focused_editor_id(&self) -> (crate::DocumentId, ::editor::EditorId) {
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
    workshop: &std::sync::Arc<::editor::Workshop>,
) -> R {
    use imba::effect::{block_on, EffectHandler};
    let payload = effect.into_payload();
    let (value, lift) = payload.split();
    let outcome: Box<dyn std::any::Any + Send + Sync> = match value
        .downcast::<::editor::RepairEffect>()
    {
        Ok(effect) => {
            let handler = ::editor::RepairHandler(std::sync::Arc::clone(workshop));
            Box::new(block_on(Box::pin(
                async move { handler.handle(*effect).await },
            )))
        }
        Err(value) => match value.downcast::<::editor::ReparseEffect>() {
            Ok(effect) => {
                let handler = ::editor::ReparseHandler(std::sync::Arc::clone(workshop));
                Box::new(block_on(Box::pin(
                    async move { handler.handle(*effect).await },
                )))
            }
            Err(value) => match value.downcast::<::editor::EnrichEffect>() {
                Ok(effect) => {
                    let handler = ::editor::EnrichHandler {
                        workshop: std::sync::Arc::clone(workshop),
                        caller: imba::effect::EffectCaller::disconnected(),
                    };
                    Box::new(block_on(Box::pin(
                        async move { handler.handle(*effect).await },
                    )))
                }
                Err(value) => match value.downcast::<::editor::RepairDiffEffect>() {
                    Ok(effect) => {
                        let handler = ::editor::RepairDiffHandler(std::sync::Arc::clone(workshop));
                        Box::new(block_on(Box::pin(
                            async move { handler.handle(*effect).await },
                        )))
                    }
                    Err(value) => match value.downcast::<crate::diffs::DiffNormalizeEffect>() {
                        Ok(effect) => {
                            let handler = crate::diffs::DiffNormalizeHandler;
                            Box::new(block_on(Box::pin(
                                async move { handler.handle(*effect).await },
                            )))
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
    };
    lift(outcome).expect("a notification lands nothing — this harness drives only landing effects")
}

pub fn test_workshop(theme: ::editor::theme::Theme) -> std::sync::Arc<::editor::Workshop> {
    std::sync::Arc::new(::editor::Workshop::new(
        ::editor::embedded_fonts::source(),
        theme,
    ))
}
