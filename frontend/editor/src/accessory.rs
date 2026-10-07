// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Editor accessories: the find bar, the completion popup, the hover
//! card — services of an EDITOR, wherever it is mounted (a pane, a
//! diff half, a canvas row's face). The shell installs them once at
//! boot; every editor view consults them on its own roads: keys
//! before the text, chrome over the viewport, a look at each command
//! before and after it performs, and the animation clock while one
//! is armed. Nothing about a pane is involved, so nothing sticks
//! when a pane swaps its editor.

use std::sync::Arc;

use imba::arena::Arena;
use imba::focus::FocusData;
use imba::store::Store;
use imba::ui::UiCtx;

use crate::document::Document;
use crate::editor::{EditorEffects, EditorId};
use crate::editor_view::EditorCommand;
use crate::location::ResourceLocation;

/// What the editor just performed, for the after-hook — the few
/// facts accessories key on, taken before the command moved.
#[derive(Clone, Debug, PartialEq)]
pub enum Performed {
    Inserted(String),
    Click,
    Hover(Option<skia_safe::Point>),
    Clock(imba::anim::AnimationClock),
    Other,
}

impl Performed {
    pub fn of(command: &EditorCommand) -> Self {
        match command {
            EditorCommand::InsertText { text } => Performed::Inserted(text.clone()),
            EditorCommand::Click { .. } => Performed::Click,
            EditorCommand::Hover(point) => Performed::Hover(*point),
            EditorCommand::AccessoryTick(now) => Performed::Clock(*now),
            _ => Performed::Other,
        }
    }
}

pub trait EditorAccessory: Send + Sync {
    /// Keys BEFORE the text, when the accessory holds them (a focused
    /// find input).
    fn focus_data<'w>(
        &self,
        store: &'w Store,
        ui: &'w UiCtx,
        editor: EditorId,
    ) -> Option<FocusData<'w, EditorCommand>> {
        let _ = (store, ui, editor);
        None
    }

    /// Chrome pinned over the top of the editor's viewport (the find
    /// bar): its height and widget.
    fn bar<'a>(
        &self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        editor: EditorId,
        width: f32,
    ) -> Option<(f32, imba::ThunkBox<'a, EditorCommand>)> {
        let _ = (arena, store, ui, editor, width);
        None
    }

    /// Wants the animation clock (a hover resting toward its ask).
    fn wants_clock(&self, store: &Store, editor: EditorId) -> bool {
        let _ = (store, editor);
        false
    }

    /// A look at a command BEFORE the editor performs it: hand it
    /// back, or eat it (`None`) — the completion popup's own commands,
    /// the find bar's, the landings addressed to the accessory.
    #[allow(clippy::too_many_arguments)]
    fn intercept(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        location: Option<&ResourceLocation>,
        command: EditorCommand,
        fx: &mut EditorEffects<'_>,
    ) -> Option<EditorCommand> {
        let _ = (store, ui, document, editor, location, fx);
        Some(command)
    }

    /// After the editor performed: keep up with what it did.
    #[allow(clippy::too_many_arguments)]
    fn after(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        location: Option<&ResourceLocation>,
        performed: &Performed,
        fx: &mut EditorEffects<'_>,
    ) {
        let _ = (store, ui, document, editor, location, performed, fx);
    }
}

/// The installed accessories — a boot resident.
#[derive(Clone, Default)]
pub struct Accessories(rpds::VectorSync<Arc<dyn EditorAccessory>>);

impl Accessories {
    pub fn install(store: &mut Store, accessory: Arc<dyn EditorAccessory>) {
        let mut installed = store
            .get::<Accessories>()
            .map(|held| held.0.clone())
            .unwrap_or_else(rpds::VectorSync::new_sync);
        installed.push_back_mut(accessory);
        store.put(Accessories(installed));
    }

    pub fn of(store: &Store) -> Vec<Arc<dyn EditorAccessory>> {
        store
            .get::<Accessories>()
            .map(|held| held.0.iter().map(Arc::clone).collect())
            .unwrap_or_default()
    }
}
