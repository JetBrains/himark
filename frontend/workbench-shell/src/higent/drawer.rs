// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The agents drawer's SHELL half: the toggle that stands the panel
//! in a window's side slot, the toolbar face, the add-host and share
//! commands — and `drawer_asks`, the verbs the panel hands the modal
//! drain without ever naming a window itself. The panel is
//! `ahp_session::session::drawer` (the view with its catalog).

use std::sync::Arc;

use ahp_session::session::drawer::{AgentsPanel, DrawerAsks};
use gfx::Rect;
use imba::store::Store;

/// The shell's answers to the panel's gestures, each closed over the

pub struct ToggleAgentsView;

impl himark::commands::HimarkCommand for ToggleAgentsView {
    fn id(&self) -> &'static str {
        "agent.toggle-agents"
    }

    fn name(&self) -> String {
        "Agents".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
            return;
        };
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
        if entity.side_panel_side() == Some(::workbench::columns::DrawerSide::Left) {
            entity.roll_away_side_panel();
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }
        fx.scope(
            move |command| crate::shell::content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        // Opening the drawer is the retry nudge a failed host waits
        // for — the subscription itself never hammers one.
        fx.follow_up(crate::app::AppCommand::Verb(imba::command::Verb::Dynamic(
            std::sync::Arc::new(ahp_session::session::driver::RetryHosts),
        )));
        use hikit::modal::Panel as _;
        let panel = AgentsPanel::open(store, ui, drawer_asks(window)).paneled();
        fx.scope(
            move |command| crate::shell::content(window, command),
            |fx| {
                entity.show_side_panel(
                    store,
                    Box::new(panel),
                    ::workbench::columns::DrawerSide::Left,
                    fx,
                )
            },
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> ::workbench::toolbar::ToolbarButton {
    ::workbench::toolbar::ToolbarButton {
        command: "agent.toggle-agents",
        label: "Sessions",
        order: 2.0,
        side: ::workbench::toolbar::ToolbarSide::Left,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = gfx::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(gfx::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(gfx::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());

            let head = Rect::from_xywh(l + w * 0.14, t + h * 0.3, w * 0.72, h * 0.56);
            canvas.draw_round_rect(head, w * 0.14, w * 0.14, &paint);
            let mut path = gfx::PathBuilder::new();
            path.move_to((l + w * 0.5, t + h * 0.3));
            path.line_to((l + w * 0.5, t + h * 0.12));
            canvas.draw_path(&path.detach(), &paint);
            let mut dot = gfx::Paint::default();
            dot.set_anti_alias(true);
            dot.set_color(color);
            canvas.draw_circle((l + w * 0.36, t + h * 0.56).into(), w * 0.055, &dot);
            canvas.draw_circle((l + w * 0.64, t + h * 0.56).into(), w * 0.055, &dot);
        }),
    }
}

pub struct ShareHost;

impl himark::commands::HimarkCommand for ShareHost {
    fn id(&self) -> &'static str {
        "host.share"
    }

    fn name(&self) -> String {
        "Share Host over HTTP".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
            return;
        };
        let client = store
            .get::<ahp_wire::client::LocalHost>()
            .and_then(|local| local.0)
            .and_then(|host| ahp_wire::client::Servers::client(store, host));
        let Some(client) = client else {
            eprintln!("[himark] share: no local host designated");
            return;
        };
        fx.push(
            imba::effect::AnyEffect::new(ahp_wire::effects::ShareHostEffect {
                client: client.session.clone(),
            })
            .map(move |result| crate::shell::windowed(window, Arc::new(SharedHost { result }))),
        );
    }
}

pub struct ShareHostTailnet;

impl himark::commands::HimarkCommand for ShareHostTailnet {
    fn id(&self) -> &'static str {
        "host.share-tailnet"
    }

    fn name(&self) -> String {
        "Share Host on Tailnet".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::app::bound_window(self.id(), bound) else {
            return;
        };
        let client = store
            .get::<ahp_wire::client::LocalHost>()
            .and_then(|local| local.0)
            .and_then(|host| ahp_wire::client::Servers::client(store, host));
        let Some(client) = client else {
            eprintln!("[himark] share-tailnet: no local host designated");
            return;
        };
        fx.push(
            imba::effect::AnyEffect::new(ahp_wire::effects::ShareTailnetEffect {
                client: client.session.clone(),
            })
            .map(move |result| crate::shell::windowed(window, Arc::new(SharedTailnet { result }))),
        );
        // Sharing enrolls the HOST's node only. The app's own client
        // node is a separate device with its own login — joining it
        // here would mean two browser logins in a row — so that stays
        // the drawer's "Add host → Connect to tailnet".
    }
}

struct SharedTailnet {
    result: Result<ahp_wire::client::TailnetShare, String>,
}

impl himark::commands::HimarkCommand for SharedTailnet {
    fn id(&self) -> &'static str {
        "host.shared-tailnet"
    }

    fn name(&self) -> String {
        "Host Shared on Tailnet".to_owned()
    }

    fn perform(
        &self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _bound: &imba::focus::Bindings,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        match &self.result {
            Ok(share) => match share.state.as_str() {
                "running" => eprintln!(
                    "[himark] tailnet: serving at {} (copied to clipboard)",
                    share.url.as_deref().unwrap_or("?")
                ),
                "needsLogin" => eprintln!(
                    "[himark] tailnet: login opened in the browser (URL also on the \
                     clipboard) — rerun after signing in"
                ),
                "waitingApproval" => {
                    eprintln!("[himark] tailnet: waiting for the tailnet admin's approval")
                }
                "failed" => eprintln!(
                    "[himark] tailnet: {}",
                    share.error.as_deref().unwrap_or("failed")
                ),
                other => eprintln!("[himark] tailnet: {other} — rerun to check progress"),
            },
            Err(error) => eprintln!("[himark] share-tailnet failed: {error}"),
        }
    }
}

struct SharedHost {
    result: Result<String, String>,
}

impl himark::commands::HimarkCommand for SharedHost {
    fn id(&self) -> &'static str {
        "host.shared"
    }

    fn name(&self) -> String {
        "Host Shared".to_owned()
    }

    fn perform(
        &self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _bound: &imba::focus::Bindings,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        match &self.result {
            Ok(url) => eprintln!("[himark] sharing at {url} (copied to clipboard)"),
            Err(error) => eprintln!("[himark] share failed: {error}"),
        }
    }
}

/// The asks of the drawer standing in a side slot — bound to the
/// window the drawer stands in.
#[doc(hidden)]
pub fn drawer_asks(window: ::workbench::window::WindowId) -> Arc<DrawerAsks> {
    Arc::new(DrawerAsks {
        open_session: Arc::new(move |store| Some(crate::workspace::window_session(store, window)?)),
        open_row: Arc::new(move |server, session| {
            himark::app::shell_verb(crate::shell::windowed(
                window,
                Arc::new(crate::higent::open_session::OpenSessionRow { server, session }),
            ))
        }),
        new_session: Arc::new(move |store, server| {
            let command: Arc<dyn himark::commands::HimarkCommand> =
                match hiahp::higent::flows::AgentFlows::new_session_flow(store) {
                    Some(flow) => flow(server),
                    None => {
                        Arc::new(crate::higent::open_session::OpenNewSession { host: Some(server) })
                    }
                };
            himark::app::shell_verb(crate::shell::windowed(window, command))
        }),
        add_host: Arc::new(move |url| {
            himark::app::shell_verb(crate::shell::windowed(
                window,
                Arc::new(hiahp::higent::flows::AddHost { url }),
            ))
        }),
    })
}

/// The drawer's FOLD-BACK, seated into `AgentFlows::install_refold`:
/// a WINDOW gesture re-opens the side slot to show the fresh host; a
/// mounted origin (no window bound) no-ops.
pub(crate) struct DrawerRefold;

impl himark::commands::HimarkCommand for DrawerRefold {
    fn id(&self) -> &'static str {
        "agent.drawer-refold"
    }
    fn name(&self) -> String {
        "Refold Agents Drawer".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        if bound.bound::<::workbench::window::WindowId>().is_some() {
            ToggleAgentsView.perform(store, ui, bound, fx);
        }
    }
}
