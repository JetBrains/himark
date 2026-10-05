// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The editor pane's SERVICES — the find bar, the completion popups:
//! shell features that ride an editor leaf without the workbench
//! knowing what they are. The shell installs ONE service face in the
//! registry; each leaf holds one opaque state box the face minted.
//! The node routes what a pane's chrome needs routed — focus, a bar
//! above the editor, command landings, the after-perform sync, the
//! editor-command intercept — and nothing else.

use std::any::Any;

use imba::arena::Arena;
use imba::dyn_view::DynCommand;
use imba::effect::Effects;
use imba::store::Store;
use imba::ui::UiCtx;

use crate::workbench_node::PanelCommand;

/// A leaf's service state, opaque to the workbench.
pub type ServiceState = Box<dyn Any + Send + Sync>;

/// A leaf's SEAT: the installed face plus this leaf's own state.
/// Carried on the slot so the slot clones without consulting the
/// store (window entities clone every content batch).
pub struct ServiceSeat {
    pub face: std::sync::Arc<dyn PaneServices>,
    pub state: ServiceState,
}

impl Clone for ServiceSeat {
    fn clone(&self) -> Self {
        Self {
            face: std::sync::Arc::clone(&self.face),
            state: self.face.clone_state(&self.state),
        }
    }
}

/// What a service acts on: the leaf's documents collection and its
/// editor target, when the leaf shows one.
#[derive(Clone, Copy)]
pub struct ServiceTarget {
    pub documents: Option<imba::store::Id<documents::OpenDocuments>>,
    pub target: Option<(documents::DocumentId, ::editor::editor::EditorId)>,
}

pub trait PaneServices: Send + Sync {
    /// Fresh state for a new leaf.
    fn mint(&self) -> ServiceState;

    /// Slots clone with their window entity — so must their state.
    fn clone_state(&self, state: &ServiceState) -> ServiceState;

    /// The services' keyboard face, when one wants the keys BEFORE
    /// the panel (a focused find bar). `None` leaves the panel's own.
    fn focus_data<'w>(
        &self,
        state: &'w ServiceState,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> Option<imba::focus::FocusData<'w, DynCommand>>;

    /// A service command (`PanelCommand::Service`) lands here.
    fn command(
        &self,
        state: &mut ServiceState,
        target: ServiceTarget,
        store: &mut Store,
        ui: &UiCtx,
        command: DynCommand,
        fx: &mut Effects<'_, PanelCommand>,
    );

    /// Intercept a panel command before the panel sees it — the
    /// completion popup eats its own inlay commands. `None` consumes.
    fn intercept(
        &self,
        state: &mut ServiceState,
        target: ServiceTarget,
        store: &mut Store,
        ui: &UiCtx,
        command: PanelCommand,
        fx: &mut Effects<'_, PanelCommand>,
    ) -> Option<PanelCommand>;

    /// The after-perform sync: the services keep up with what the
    /// editor just did (`inserted` carries freshly typed text).
    fn sync(
        &self,
        state: &mut ServiceState,
        target: ServiceTarget,
        store: &mut Store,
        ui: &UiCtx,
        inserted: Option<&str>,
        fx: &mut Effects<'_, PanelCommand>,
    );

    /// A pointer press landed in the panel: bars lose the keyboard.
    fn defocus(&self, state: &mut ServiceState);

    /// The chrome ABOVE the editor, when a service stands one (the
    /// find bar): its height and its widget.
    fn bar<'a>(
        &self,
        state: &'a ServiceState,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        width: f32,
    ) -> Option<(f32, imba::ThunkBox<'a, DynCommand>)>;
}
