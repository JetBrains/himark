// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The terminal feature: the open command, the AHP-backed pty
//! session, and the pane that shows it. Moved here from the host —
//! the one host service it needs (posting a refresh from the pty's
//! event thread) rides the engine's [`himark::effects::Poster`].

use std::sync::Arc;

use imba::effect::{AnyEffect, Effect, EffectHandler};

use himark::commands::HimarkCommand;

/// The terminal feature as a plugin: the command and the pty
/// handler. The pane the landing opens is WINDOW furniture — the
/// command stays on the window road until a terminal place exists
/// for the mounted shells.
pub struct ChromePlugin;

impl himark::plugin::Plugin for ChromePlugin {
    fn install(&self, app: &mut himark::app::Himark, _cx: &himark::plugin::PluginContext) {
        use himark::app_ext::AppExt;
        app.register_handler::<NewTerminalEffect>(SessionTerminalHandler {
            poster: app.poster(),
        });
        app.register_command(Arc::new(OpenTerminal));
    }
}

pub struct NewTerminalEffect {
    pub(crate) client: Arc<dyn ahp_wire::client::TerminalClient>,
    /// The session the terminal is spawned IN — the wire's address.
    pub(crate) home: ahp_wire::SessionId,
    pub(crate) cwd: Option<String>,
    pub(crate) window: ::workbench::window::WindowId,
}

impl std::fmt::Display for NewTerminalEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "new terminal {}", self.home.session.as_str())
    }
}

impl Effect for NewTerminalEffect {
    type Result = Option<Arc<terminals::Session>>;
}

pub(crate) struct SessionTerminalHandler {
    /// The pty's events arrive on the wire's thread — the repaint
    /// ask crosses to the UI loop through the engine's poster.
    pub(crate) poster: himark::effects::Poster,
}

struct AhpBackend {
    client: Arc<dyn ahp_wire::client::TerminalClient>,
    channel: ahp_wire::client::ChannelUri,
}

impl terminals::TerminalBackend for AhpBackend {
    fn write(&self, bytes: &[u8]) {
        self.client
            .terminal_input(&self.channel, String::from_utf8_lossy(bytes).into_owned());
    }
    fn resize(&self, cols: u16, rows: u16, _px_width: f32, _px_height: f32) {
        self.client.terminal_resize(&self.channel, cols, rows);
    }
    fn hangup(&self) {
        self.client.terminal_dispose(&self.channel);
    }
}

impl EffectHandler<NewTerminalEffect> for SessionTerminalHandler {
    async fn handle(&self, effect: NewTerminalEffect) -> Option<Arc<terminals::Session>> {
        let channel =
            ahp_wire::client::ChannelUri::new(format!("ahp-terminal:/{}", ahp_wire::uuid_v4()));
        let session = terminals::Session::new(Box::new(AhpBackend {
            client: Arc::clone(&effect.client),
            channel: channel.clone(),
        }));
        let pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let events = {
            let session = Arc::clone(&session);
            let poster = self.poster.clone();
            let window = effect.window;
            let pending = Arc::clone(&pending);
            Arc::new(move |event: ahp_wire::client::TerminalEvent| {
                match event {
                    ahp_wire::client::TerminalEvent::Data(text) => {
                        let _ = session.output(text.as_bytes());
                    }
                    ahp_wire::client::TerminalEvent::Exited(code) => {
                        let _ = session.exited(code.unwrap_or(-1));
                    }
                    // A reconnect replays the host's snapshot from
                    // byte one — wipe the grid first.
                    ahp_wire::client::TerminalEvent::Reset => {
                        let _ = session.reset();
                    }
                }

                if !pending.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    poster.post(himark::shell::windowed(
                        window,
                        Arc::new(RefreshTerminal(Arc::clone(&pending))),
                    ));
                }
            }) as Arc<dyn Fn(ahp_wire::client::TerminalEvent) + Send + Sync>
        };
        effect
            .client
            .terminal_open(
                effect.home.session.clone(),
                channel,
                effect.cwd.clone(),
                80,
                24,
                events,
            )
            .await?;
        Some(session)
    }
}

pub(crate) struct OpenTerminal;

impl HimarkCommand for OpenTerminal {
    fn id(&self) -> &'static str {
        "terminal.open"
    }
    fn name(&self) -> String {
        "New Terminal".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        // The pane the landing opens is WINDOW furniture — this
        // command stays on the window road until a terminal place
        // exists for the mounted shells.
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
        let Some(entity) = ::workbench::window::Windows::window(store, window) else {
            return;
        };
        let workspace = himark::workspace::entity_session(&entity);
        ::workbench::window::Windows::put(store, window, entity);
        let resolved = ahp_session::session::agents::Agents::live_session(store, &workspace)
            .and_then(|key| {
                let client = ahp_wire::client::Servers::client(store, key.host)?;
                let cwd = ahp_session::session::agents::Agents::record(store, key.host).and_then(
                    |record| {
                        record.summary(&key.session).and_then(|summary| {
                            summary.working_directories.as_ref()?.first().cloned()
                        })
                    },
                );
                Some((client, key.clone(), cwd))
            })
            .or_else(|| {
                let server = store
                    .get::<ahp_wire::client::LocalHost>()
                    .copied()
                    .unwrap_or_default()
                    .0?;
                let client = ahp_wire::client::Servers::client(store, server)?;
                let cwd = ahp_session::session::folders::session_folders(store, &workspace)
                    .first()
                    .map(|folder| {
                        ahp_wire::client::ResourceUriMap::uri_of(&ahp_wire::uris::FileUris, folder)
                            .into_string()
                    });
                Some((
                    client,
                    ahp_wire::SessionId {
                        host: server,
                        session: ahp_wire::client::SessionUri::new(
                            host_discovery::LOCAL_FS_SESSION,
                        ),
                    },
                    cwd,
                ))
            });
        let Some((client, home, cwd)) = resolved else {
            return;
        };
        // The terminal files into the WINDOW's session — the collection
        // the pane is minted against, carried from here to the landing.
        let Some(terminals) =
            himark::workspace::session_state(store, window).map(|state| state.terminals())
        else {
            return;
        };
        let _ = fx.push(
            AnyEffect::new(NewTerminalEffect {
                client: client.terminals.clone(),
                home,
                cwd,
                window,
            })
            .map(move |session| {
                himark::shell::windowed(window, Arc::new(ShowTerminal { session, terminals }))
            }),
        );
    }
}

struct ShowTerminal {
    session: Option<Arc<terminals::Session>>,
    terminals: imba::store::Id<terminals::Terminals>,
}

impl HimarkCommand for ShowTerminal {
    fn id(&self) -> &'static str {
        "terminal.show"
    }
    fn name(&self) -> String {
        "Show Terminal".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
        let Some(session) = &self.session else {
            return;
        };
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");

        let terminals = self.terminals;
        let id = terminals::TerminalId::mint();
        terminals::Terminals::put(store, terminals, id, session.clone());
        if !entity.open_panel(
            store,
            ui,
            Box::new(terminals::pane::TerminalView::new(terminals, id)),
            fx,
        ) {
            session.hangup();
            terminals::Terminals::remove(store, terminals, id);
        }
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub(crate) struct RefreshTerminal(pub(crate) Arc<std::sync::atomic::AtomicBool>);

impl HimarkCommand for RefreshTerminal {
    fn id(&self) -> &'static str {
        "terminal.refresh"
    }
    fn name(&self) -> String {
        String::new()
    }
    fn perform(
        &self,
        _store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _bound: &imba::focus::Bindings,
        _fx: &mut himark::app::AppFx<'_>,
    ) {
        // The post itself is the repaint ask: the drain that carries
        // this command redraws every view. Clearing the flag re-arms
        // the pty's coalescing.
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
