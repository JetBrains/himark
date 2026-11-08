// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The terminal feature OFF the window road: the open command
//! resolves its session from the BINDINGS (window and mounted
//! walks both bind it), the pty handler posts its repaints as
//! plain engine verbs, and the landing navigates the ORIGIN to a
//! ready [`TerminalViewPlace`] — the workbench's arm opens a
//! panel, the plugin's bridge arm mounts the pane for a native
//! shell.

use std::sync::Arc;

use imba::effect::{AnyEffect, Effect, EffectHandler};
use imba::store::Store;
use imba::ui::UiCtx;

use himark::app::{AppCommand, AppFx};
use himark::commands::{bound_origin, HimarkCommand};

/// A minted terminal, ready to show — the ready place the open's
/// landing hands the shell (the terminal twin of `DiffViewPlace`).
#[derive(Clone, PartialEq)]
pub struct TerminalViewPlace {
    pub terminals: imba::store::Id<terminals::Terminals>,
    pub id: terminals::TerminalId,
}

impl hikit::navigation::Place for TerminalViewPlace {}

/// The terminal feature as a plugin: the command, the pty handler,
/// and the mounted arm for the ready place.
pub struct TerminalPlugin;

impl himark::plugin::Plugin for TerminalPlugin {
    fn install(&self, app: &mut himark::app::Himark, _cx: &himark::plugin::PluginContext) {
                app.register_handler::<NewTerminalEffect>(SessionTerminalHandler {
            poster: app.poster(),
        });
        app.perform_batch(vec![himark::app::AppCommand::Register(Arc::new(OpenTerminal))]);
        // The MOUNTED arm: a native shell shows the ready terminal
        // as its own bridge.
        app.register_view_bridge_factory(Arc::new(|app, place| {
            let place = place.place::<TerminalViewPlace>()?.clone();
            app.mount(move |_store, _ui, id, _fx| {
                Some(himark::mounts::panel_mount(
                    id,
                    Box::new(terminals::pane::TerminalView::new(place.terminals, place.id)),
                ))
            })
        }));
    }
}

pub struct NewTerminalEffect {
    client: Arc<dyn ahp_wire::client::TerminalClient>,
    /// The session the terminal is spawned IN — the wire's address.
    home: ahp_wire::SessionId,
    cwd: Option<String>,
}

impl std::fmt::Display for NewTerminalEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "new terminal {}", self.home.session.as_str())
    }
}

impl Effect for NewTerminalEffect {
    type Result = Option<Arc<terminals::Session>>;
}

struct SessionTerminalHandler {
    /// The pty's events arrive on the wire's thread — the repaint
    /// ask crosses to the UI loop through the engine's poster.
    poster: himark::effects::Poster,
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

                // The post itself is the repaint ask: the drain that
                // carries it redraws every view — a plain engine
                // verb, no window, no origin.
                if !pending.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    poster.post(AppCommand::Verb(imba::command::Verb::once(
                        RefreshTerminal(Arc::clone(&pending)),
                    )));
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

struct OpenTerminal;

impl HimarkCommand for OpenTerminal {
    fn id(&self) -> &'static str {
        "terminal.open"
    }
    fn name(&self) -> String {
        "New Terminal".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut AppFx<'_>,
    ) {
        // The workspace rides the BINDINGS — the window walk binds
        // its entity's session, the mounted binder binds the
        // mount's: one road, no window anywhere.
        let Some(workspace) = bound.bound::<ahp_wire::SessionId>().cloned() else {
            eprintln!("[terminal] no session bound — dropped: terminal.open");
            return;
        };
        let Some(origin) = bound_origin(self.id(), bound) else {
            return;
        };
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
        // The terminal files into the WORKSPACE's session — the
        // collection the pane is minted against.
        let Some(terminals) = hiahp::bound_session(self.id(), bound).map(|state| state.terminals())
        else {
            return;
        };
        let _ = fx.push(
            AnyEffect::new(NewTerminalEffect {
                client: client.terminals.clone(),
                home,
                cwd,
            })
            .map(move |session| {
                AppCommand::AtOrigin(origin, Arc::new(ShowTerminal { session, terminals }))
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
        store: &mut Store,
        ui: &UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut AppFx<'_>,
    ) {
        let Some(origin) = bound_origin(self.id(), bound) else {
            return;
        };
        let Some(session) = &self.session else {
            return;
        };
        let terminals = self.terminals;
        let id = terminals::TerminalId::mint();
        terminals::Terminals::put(store, terminals, id, session.clone());
        // The READY place to the seated shell: the workbench's arm
        // opens the panel, a native shell mints the bridge. A shell
        // that declines leaves a filed terminal behind — the reap
        // follows the collection, not this landing.
        fx.scope(AppCommand::Verb, |fx| {
            himark::shell::navigate_origin(
                store,
                ui,
                origin,
                TerminalViewPlace { terminals, id },
                fx,
            )
        });
    }
}

/// The repaint ask, as a once-verb: performing it IS the redraw
/// (the drain invalidates); clearing the flag re-arms the pty's
/// coalescing.
struct RefreshTerminal(Arc<std::sync::atomic::AtomicBool>);

impl imba::command::DynamicOnceCommand for RefreshTerminal {
    fn perform(self: Box<Self>, _store: &mut Store, _ui: &UiCtx, _fx: &mut imba::command::Fx<'_>) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
