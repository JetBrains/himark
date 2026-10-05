// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The agents drawer's SHELL half: the toggle that stands the panel
//! in a window's side slot, the toolbar face, the add-host and share
//! commands — and `drawer_asks`, the verbs the panel hands the modal
//! drain without ever naming a window itself. The panel is
//! `ahp_session::session::drawer` (the view with its catalog).

use std::sync::Arc;

use ahp_session::session::drawer::{AgentsPanel, DrawerAsks};
use crate::app::AppCommand;
use crate::higent::open_session::OpenSessionRow;
use imba::store::Store;
use skia_safe::Rect;

/// The shell's answers to the panel's gestures, each closed over the
/// window the drawer stands in.
pub fn drawer_asks(window: ::workbench::window::WindowId) -> Arc<DrawerAsks> {
    Arc::new(DrawerAsks {
        open_session: Arc::new(move |store| {
            Some(crate::grip::window_session(store, window)?)
        }),
        open_row: Arc::new(move |server, session| {
            crate::app::shell_verb(AppCommand::Windowed(
                window,
                Arc::new(OpenSessionRow { server, session }),
            ))
        }),
        new_session: Arc::new(move |store, server| {
            let command: Arc<dyn crate::commands::WindowedCommand> =
                match crate::higent::flows::AgentFlows::new_session_flow(store) {
                    Some(flow) => flow(server),
                    None => Arc::new(crate::new_session::OpenNewSession { host: Some(server) }),
                };
            crate::app::shell_verb(AppCommand::Windowed(window, command))
        }),
        add_host: Arc::new(move |url| {
            crate::app::shell_verb(AppCommand::Windowed(window, Arc::new(AddHost { url })))
        }),
    })
}

pub struct AddHost {
    pub url: String,
}

impl crate::commands::WindowedCommand for AddHost {
    fn id(&self) -> &'static str {
        "agent.add-host"
    }

    fn name(&self) -> String {
        "Add Host".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(flow) = crate::higent::flows::AgentFlows::add_host_flow(store) else {
            eprintln!("[higent] no add-host capability installed — url dropped");
            return;
        };
        if flow(store, &self.url).is_none() {
            return;
        }
        ToggleAgentsView.perform(store, ui, window, fx);
    }
}

pub struct ToggleAgentsView;

impl crate::commands::WindowedCommand for ToggleAgentsView {
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
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        if entity.has_side_panel() {
            entity.roll_away_side_panel();
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }
        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        // Opening the drawer is the retry nudge a failed host waits
        // for — the subscription itself never hammers one.
        fx.follow_up(crate::app::AppCommand::Verb(imba::command::Verb::Dynamic(
            std::sync::Arc::new(ahp_session::session::driver::RetryHosts),
        )));
        let panel = AgentsPanel::open(store, ui, drawer_asks(window));
        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.show_side_panel(store, Box::new(panel), fx),
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> ::workbench::toolbar::ToolbarButton {
    ::workbench::toolbar::ToolbarButton {
        command: "agent.toggle-agents",
        order: 2.0,
        side: ::workbench::toolbar::ToolbarSide::Left,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());

            let head = Rect::from_xywh(l + w * 0.14, t + h * 0.3, w * 0.72, h * 0.56);
            canvas.draw_round_rect(head, w * 0.14, w * 0.14, &paint);
            let mut path = skia_safe::PathBuilder::new();
            path.move_to((l + w * 0.5, t + h * 0.3));
            path.line_to((l + w * 0.5, t + h * 0.12));
            canvas.draw_path(&path.detach(), &paint);
            let mut dot = skia_safe::Paint::default();
            dot.set_anti_alias(true);
            dot.set_color(color);
            canvas.draw_circle((l + w * 0.36, t + h * 0.56), w * 0.055, &dot);
            canvas.draw_circle((l + w * 0.64, t + h * 0.56), w * 0.055, &dot);
        }),
    }
}

pub struct ShareHost;

impl crate::commands::WindowedCommand for ShareHost {
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
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let client = store
            .get::<ahp_wire::client::LocalHost>()
            .and_then(|local| local.0)
            .and_then(|host| ahp_wire::client::Servers::client(store, host));
        let Some(client) = client else {
            eprintln!("[himark] share: no local host designated");
            return;
        };
        fx.push(
            imba::effect::AnyEffect::new(ahp_wire::effects::ShareHostEffect { client: client.session.clone() })
                .map(move |result| AppCommand::Windowed(window, Arc::new(SharedHost { result }))),
        );
    }
}

struct SharedHost {
    result: Result<String, String>,
}

impl crate::commands::WindowedCommand for SharedHost {
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
        _window: ::workbench::window::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        match &self.result {
            Ok(url) => eprintln!("[himark] sharing at {url} (copied to clipboard)"),
            Err(error) => eprintln!("[himark] share failed: {error}"),
        }
    }
}
