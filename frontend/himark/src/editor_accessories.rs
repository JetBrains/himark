// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The editor's accessories (`editor::accessory`): the find bar, the
//! completion popup, the hover card. One SEAT per editor, in the
//! store; one installed accessory serves every editor view on its own
//! roads — a pane, a diff half, a canvas row's face alike. Nothing
//! here knows a pane, so nothing sticks when a pane swaps its editor.

use std::sync::Arc;

use ::editor::accessory::{Accessories, EditorAccessory, Performed};
use ::editor::document::Document;
use ::editor::dynamic::{DynPayload, DynamicEditorCommand, EditorCommands};
use ::editor::editor::{EditorEffects, EditorId};
use ::editor::editor_view::EditorCommand;
use ::editor::location::ResourceLocation;
use ahp_chat::completion::{Completion, CompletionCommand, CompletionFound};
use documents::hover::{Hover, HoverFound};
use findbar::{FindBar, FindCommand};
use imba::store::Store;
use imba::thunk_ext::ThunkExt as _;
use imba::ui::UiCtx;

/// The find bar's commands ride the editor's dynamic road under this
/// id; the landings addressed to an accessory ride their own.
const FIND: &str = "find.command";
const COMPLETION_FOUND: &str = "completion.found";
const HOVER_FOUND: &str = "hover.found";

/// One editor's accessories.
#[derive(Clone)]
pub struct Seat {
    pub find: Option<FindBar>,
    pub completion: Completion,
    pub hover: Hover,
}

impl Default for Seat {
    fn default() -> Self {
        Self {
            find: None,
            completion: Completion::new(),
            hover: Hover::new(),
        }
    }
}

/// The seats, by editor — a boot resident.
#[derive(Clone, Default)]
pub struct Seats(rpds::HashTrieMapSync<EditorId, Seat>);

impl Seats {
    pub fn seat(store: &Store, editor: EditorId) -> Option<&Seat> {
        store.get::<Seats>()?.0.get(&editor)
    }

    fn take(store: &Store, editor: EditorId) -> Seat {
        Self::seat(store, editor).cloned().unwrap_or_default()
    }

    fn put(store: &mut Store, editor: EditorId, seat: Seat) {
        let mut seats = store.get::<Seats>().cloned().unwrap_or_default();
        seats.0.insert_mut(editor, seat);
        store.put(seats);
    }

    /// Mutate an editor's seat in place (minting it on first touch).
    pub fn update(store: &mut Store, editor: EditorId, mutate: impl FnOnce(&mut Seat)) {
        let mut seat = Self::take(store, editor);
        mutate(&mut seat);
        Self::put(store, editor, seat);
    }
}

fn find_command(command: FindCommand) -> EditorCommand {
    EditorCommand::Dynamic {
        id: FIND,
        payload: Some(DynPayload::new(command)),
    }
}

fn completion_found(found: CompletionFound) -> EditorCommand {
    EditorCommand::Dynamic {
        id: COMPLETION_FOUND,
        payload: Some(DynPayload::new(found)),
    }
}

fn hover_found(found: HoverFound) -> EditorCommand {
    EditorCommand::Dynamic {
        id: HOVER_FOUND,
        payload: Some(DynPayload::new(found)),
    }
}

fn to_editor(command: EditorCommand) -> EditorCommand {
    command
}

fn payload_of<T: 'static>(payload: Option<DynPayload>) -> Option<T> {
    payload
        .and_then(DynPayload::take)
        .and_then(|boxed| boxed.downcast::<T>().ok())
        .map(|boxed| *boxed)
}

/// Boot: the accessory and the commands that drive it.
pub fn install(store: &mut Store) {
    Accessories::install(store, Arc::new(Services));
    EditorCommands::register(store, Arc::new(FindOpen));
    EditorCommands::register(store, Arc::new(CompletionTrigger));
    EditorCommands::register(store, Arc::new(FindStep(true)));
    EditorCommands::register(store, Arc::new(FindStep(false)));
}

struct Services;

impl EditorAccessory for Services {
    fn focus_data<'w>(
        &self,
        store: &'w Store,
        ui: &'w UiCtx,
        editor: EditorId,
    ) -> Option<imba::focus::FocusData<'w, EditorCommand>> {
        let find = Seats::seat(store, editor)?
            .find
            .as_ref()
            .filter(|find| find.focused)?;
        Some(find.focus_data(store, ui).map(find_command))
    }

    fn bar<'a>(
        &self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        editor: EditorId,
        width: f32,
    ) -> Option<(f32, imba::ThunkBox<'a, EditorCommand>)> {
        let find = Seats::seat(store, editor)?.find.as_ref()?;
        let chrome = ::editor::env::Themes::of(store).ui().search.clone();
        let height = FindBar::height(&chrome);
        let widget = imba::ThunkBox::new(
            arena,
            find.layout(arena, store, ui, width).map(find_command),
        );
        Some((height, widget))
    }

    fn wants_clock(&self, store: &Store, editor: EditorId) -> bool {
        Seats::seat(store, editor).is_some_and(|seat| seat.hover.armed())
    }

    fn intercept(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        _location: Option<&ResourceLocation>,
        command: EditorCommand,
        fx: &mut EditorEffects<'_>,
    ) -> Option<EditorCommand> {
        match command {
            EditorCommand::Dynamic { id: FIND, payload } => {
                if let Some(command) = payload_of::<FindCommand>(payload) {
                    let mut seat = Seats::take(store, editor);
                    perform_find(&mut seat, store, ui, document, editor, command, fx);
                    Seats::put(store, editor, seat);
                }
                None
            }
            EditorCommand::Dynamic {
                id: COMPLETION_FOUND,
                payload,
            } => {
                if let Some(found) = payload_of::<CompletionFound>(payload) {
                    let mut seat = Seats::take(store, editor);
                    seat.completion.land(store, ui, document, editor, found);
                    Seats::put(store, editor, seat);
                }
                None
            }
            EditorCommand::Dynamic {
                id: HOVER_FOUND,
                payload,
            } => {
                if let Some(found) = payload_of::<HoverFound>(payload) {
                    let mut seat = Seats::take(store, editor);
                    seat.hover
                        .land(store, ui, document, editor, found, fx, to_editor);
                    Seats::put(store, editor, seat);
                }
                None
            }
            EditorCommand::Inlay { key, command }
                if Seats::seat(store, editor)
                    .is_some_and(|seat| seat.completion.inlay_key() == Some(key)) =>
            {
                let popup = match command.downcast_ref::<CompletionCommand>() {
                    Some(_) => command
                        .downcast::<CompletionCommand>()
                        .expect("probed above"),
                    None => return Some(EditorCommand::Inlay { key, command }),
                };
                let mut seat = Seats::take(store, editor);
                match popup {
                    CompletionCommand::Select(delta) => {
                        seat.completion.select(store, document, editor, delta);
                    }
                    CompletionCommand::PickCursor => {
                        let row = seat.completion.selected();
                        let _ = seat
                            .completion
                            .apply_pick(store, ui, document, editor, row, fx, to_editor);
                    }
                    CompletionCommand::Rows(rows) => {
                        if let Some(row) = seat
                            .completion
                            .rows_command(store, ui, document, editor, rows)
                        {
                            let _ = seat
                                .completion
                                .apply_pick(store, ui, document, editor, row, fx, to_editor);
                        }
                    }
                    CompletionCommand::Close => {
                        seat.completion
                            .drop_state(document, store, ui, fx, to_editor);
                    }
                }
                Seats::put(store, editor, seat);
                None
            }
            command => Some(command),
        }
    }

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
        // An editor with no seat yet only gets one from the roads that
        // start something: typing (a trigger char) or a hover.
        if Seats::seat(store, editor).is_none()
            && matches!(performed, Performed::Other | Performed::Click | Performed::Clock(_))
        {
            return;
        }
        let mut seat = Seats::take(store, editor);
        let fonts = ::editor::env::ui_collection(store, ui);
        let theme = ::editor::env::Themes::of(store);
        match performed {
            Performed::Click => {
                // A pointer press lands in the text: the bar loses the
                // keyboard.
                if let Some(find) = &mut seat.find {
                    find.focused = false;
                }
                sync_completion(&mut seat, store, ui, document, editor, location, None, fx);
            }
            Performed::Hover(point) => match (point, location) {
                (Some(point), Some(location)) if !location.is_synthetic() => {
                    let byte =
                        document.byte_at_point(editor, point.x, point.y, store, ui, &fonts, &theme);
                    seat.hover.sync(
                        store, ui, document, editor, byte, location, None, fx, to_editor,
                    );
                }
                _ => {
                    if seat.hover.open() || seat.hover.armed() {
                        seat.hover.retract(store, ui, document, fx, to_editor);
                    }
                }
            },
            Performed::Clock(now) => {
                seat.hover.tick(document, *now, fx, hover_found);
            }
            Performed::Inserted(text) => {
                sync_completion(
                    &mut seat,
                    store,
                    ui,
                    document,
                    editor,
                    location,
                    Some(text.as_str()),
                    fx,
                );
            }
            Performed::Other => {
                sync_completion(&mut seat, store, ui, document, editor, location, None, fx);
            }
        }
        // The match tints follow the text.
        if let Some(find) = &mut seat.find {
            find.sync(store, document, ui, &fonts, &theme, fx);
            find.launch(document, fx, |scan| {
                find_command(FindCommand::Scanned(scan))
            });
        }
        Seats::put(store, editor, seat);
    }
}

#[allow(clippy::too_many_arguments)]
fn perform_find(
    seat: &mut Seat,
    store: &mut Store,
    ui: &UiCtx,
    document: &mut Document,
    _editor: EditorId,
    command: FindCommand,
    fx: &mut EditorEffects<'_>,
) {
    let fonts = ::editor::env::ui_collection(store, ui);
    let theme = ::editor::env::Themes::of(store);
    match command {
        FindCommand::Input(command) => {
            if let Some(find) = &mut seat.find {
                fx.scope(find_command, |fx| {
                    find.perform_input(store, ui, command, fx)
                });
                find.sync(store, document, ui, &fonts, &theme, fx);
                find.launch(document, fx, |scan| {
                    find_command(FindCommand::Scanned(scan))
                });
            }
        }
        FindCommand::Next | FindCommand::Previous => {
            let forward = matches!(command, FindCommand::Next);
            if let Some(find) = &mut seat.find {
                find.sync(store, document, ui, &fonts, &theme, fx);
                find.launch(document, fx, |scan| {
                    find_command(FindCommand::Scanned(scan))
                });
                find.step(store, document, forward, ui, &fonts, &theme, fx);
            }
        }
        FindCommand::Close => {
            if let Some(mut find) = seat.find.take() {
                find.uninstall(store, document, ui, &fonts, &theme, fx);
            }
        }
        FindCommand::Scanned(landed) => {
            if let Some(find) = &mut seat.find {
                find.adopt(store, document, _editor, &landed, ui, &fonts, &theme, fx);
            }
        }
    }
}

/// The session a located document lives in — the markdown path
/// completion's folders and recents.
fn home_of(
    store: &Store,
    location: &ResourceLocation,
) -> Option<(
    ahp_wire::SessionId,
    ahp_session::session::state::SessionState,
)> {
    ahp_session::session::state::Hosts::states(store)
        .into_iter()
        .find(|state| {
            documents::OpenDocuments::by_location(store, state.documents(), location).is_some()
        })
        .and_then(|state| {
            ahp_session::session::state::Hosts::home_of_documents(store, state.documents())
        })
}

#[allow(clippy::too_many_arguments)]
fn sync_completion(
    seat: &mut Seat,
    store: &mut Store,
    ui: &UiCtx,
    document: &mut Document,
    editor: EditorId,
    location: Option<&ResourceLocation>,
    inserted: Option<&str>,
    fx: &mut EditorEffects<'_>,
) {
    if !seat.completion.open() && inserted.is_none() {
        return;
    }
    let markdown = document.syntax().map(|syntax| syntax.language.as_str()) == Some("markdown");
    match location {
        Some(location) if markdown => {
            let Some((session, state)) = home_of(store, location) else {
                return;
            };
            let typed_at =
                (inserted == Some("@")).then(|| document.caret_byte(editor).saturating_sub(1));
            seat.completion.sync_path(
                store,
                ui,
                document,
                editor,
                typed_at,
                Arc::new(ahp_session::session::folders::session_folders(
                    store, &session,
                )),
                state.recents(),
                None,
                fx,
                completion_found,
                to_editor,
            );
        }
        Some(location) if !location.is_synthetic() => {
            seat.completion.sync_lsp(
                store,
                ui,
                document,
                editor,
                inserted,
                false,
                location,
                None,
                fx,
                completion_found,
                to_editor,
            );
        }
        _ if seat.completion.open() => {
            seat.completion
                .drop_state(document, store, ui, fx, to_editor);
        }
        _ => {}
    }
}

/// ⌘F: open the bar on this editor, seeded from a short one-line
/// selection, or refocus the standing one.
pub struct FindOpen;

impl DynamicEditorCommand for FindOpen {
    fn id(&self) -> &'static str {
        "find.open"
    }
    fn name(&self) -> String {
        "Find in Document".to_owned()
    }
    fn offers_at(&self, _location: &ResourceLocation) -> bool {
        true
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        _location: &ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut EditorEffects<'_>,
    ) {
        let seed = {
            let caret = document.carets(editor).primary();
            caret
                .has_selection()
                .then(|| caret.selection())
                .and_then(|selection| {
                    if selection.end - selection.start > 200 {
                        return None;
                    }
                    let text = document.text().view().substring(selection);
                    (!text.contains('\n')).then_some(text)
                })
        };
        let mut seat = Seats::take(store, editor);
        match (&mut seat.find, seed) {
            (Some(find), Some(seed)) => find.seed(store, ui, &seed),
            (Some(find), None) => find.refocus(),
            (None, seed) => {
                let mut find = FindBar::new(store, ui);
                if let Some(seed) = &seed {
                    find.seed(store, ui, seed);
                }
                seat.find = Some(find);
            }
        }
        let fonts = ::editor::env::ui_collection(store, ui);
        let theme = ::editor::env::Themes::of(store);
        if let Some(find) = &mut seat.find {
            find.sync(store, document, ui, &fonts, &theme, fx);
            find.launch(document, fx, |scan| {
                find_command(FindCommand::Scanned(scan))
            });
        }
        Seats::put(store, editor, seat);
    }
}

pub struct FindStep(pub bool);

impl DynamicEditorCommand for FindStep {
    fn id(&self) -> &'static str {
        match self.0 {
            true => "find.next",
            false => "find.previous",
        }
    }
    fn name(&self) -> String {
        match self.0 {
            true => "Find Next",
            false => "Find Previous",
        }
        .to_owned()
    }
    fn offers_at(&self, _location: &ResourceLocation) -> bool {
        true
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        _location: &ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut EditorEffects<'_>,
    ) {
        let mut seat = Seats::take(store, editor);
        let command = match self.0 {
            true => FindCommand::Next,
            false => FindCommand::Previous,
        };
        perform_find(&mut seat, store, ui, document, editor, command, fx);
        Seats::put(store, editor, seat);
    }
}

/// The explicit completion ask (ctrl-space) at the caret.
pub struct CompletionTrigger;

impl DynamicEditorCommand for CompletionTrigger {
    fn id(&self) -> &'static str {
        "completion.trigger"
    }
    fn name(&self) -> String {
        "Trigger Completion".to_owned()
    }
    /// Offered everywhere (the palette roster is stable); the ask
    /// itself only goes where a language server can answer.
    fn offers_at(&self, _location: &ResourceLocation) -> bool {
        true
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        location: &ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut EditorEffects<'_>,
    ) {
        let markdown = document.syntax().map(|syntax| syntax.language.as_str()) == Some("markdown");
        if markdown || location.is_synthetic() {
            return;
        }
        let mut seat = Seats::take(store, editor);
        seat.completion.sync_lsp(
            store,
            ui,
            document,
            editor,
            None,
            true,
            location,
            None,
            fx,
            completion_found,
            to_editor,
        );
        Seats::put(store, editor, seat);
    }
}
