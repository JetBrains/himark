// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! A desktop run target for the UI library, with no editor or agent host.
use std::{error::Error, num::NonZeroU32, sync::Arc};

use himark::gallery::{Gallery, GalleryMode};
use imba::event::{Event, MouseButton};
use skia_safe::{surfaces, AlphaType, ColorType, ImageInfo, Point, Size};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut mode = GalleryMode::Interactive;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--interactive" => mode = GalleryMode::Interactive,
            "--all-states" => mode = GalleryMode::AllStates,
            "--help" | "-h" => {
                println!("Himark UI gallery\n\nUsage: gallery [--interactive | --all-states]\n\nSwitch modes with the buttons or Tab. Scroll with the wheel or Page Up/Down.\nHome/End jump to the beginning/end; Escape closes the gallery.");
                return Ok(());
            }
            _ => return Err(format!("unknown gallery option: {arg}; use --help").into()),
        }
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let context = softbuffer::Context::new(event_loop.owned_display_handle())?;
    let mut host = Host {
        context,
        window: None,
        surface: None,
        gallery: Gallery::new(mode),
        cursor: Point::default(),
        scroll: 0.0,
        error: None,
    };
    event_loop.run_app(&mut host)?;
    if let Some(error) = host.error {
        return Err(error);
    }
    Ok(())
}

struct Host {
    context: softbuffer::Context<OwnedDisplayHandle>,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<OwnedDisplayHandle, Arc<Window>>>,
    gallery: Gallery,
    cursor: Point,
    scroll: f32,
    error: Option<Box<dyn Error>>,
}

impl Host {
    fn size(&self) -> Size {
        let window = self.window.as_ref().expect("resumed");
        let size = window.inner_size().to_logical::<f32>(window.scale_factor());
        Size::new(size.width, size.height)
    }

    fn clamp_scroll(&mut self) {
        let size = self.size();
        self.scroll = self.scroll.clamp(
            0.0,
            (self.gallery.content_height(size.width) - size.height).max(0.0),
        );
    }

    fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        self.clamp_scroll();
        let window = self.window.as_ref().expect("resumed");
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        let logical = self.size();
        let surface = self.surface.as_mut().expect("resumed");
        surface.resize(width, height)?;
        let mut buffer = surface.buffer_mut()?;
        // softbuffer's 0x00RRGGBB u32 pixels are BGRA bytes on the desktop
        // targets, matching the renderer used by the main winit shell.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), buffer.len() * 4)
        };
        let info = ImageInfo::new(
            (size.width as i32, size.height as i32),
            ColorType::BGRA8888,
            AlphaType::Opaque,
            None,
        );
        let mut raster = surfaces::wrap_pixels(&info, bytes, size.width as usize * 4, None)
            .ok_or("could not wrap gallery pixels")?;
        let scale = window.scale_factor() as f32;
        raster.canvas().scale((scale, scale));
        self.gallery.draw(raster.canvas(), logical, self.scroll);
        drop(raster);
        window.pre_present_notify();
        buffer.present()?;
        Ok(())
    }
}

impl ApplicationHandler for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let result = (|| -> Result<(), Box<dyn Error>> {
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("Himark · UI gallery")
                        .with_inner_size(LogicalSize::new(1100.0, 860.0))
                        .with_min_inner_size(LogicalSize::new(960.0, 480.0)),
                )?,
            );
            self.surface = Some(softbuffer::Surface::new(&self.context, window.clone())?);
            window.request_redraw();
            self.window = Some(window);
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.draw() {
                    self.error = Some(error);
                    event_loop.exit();
                }
                return;
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {}
            WindowEvent::CursorMoved { position, .. } => {
                let point = position.to_logical::<f32>(window.scale_factor());
                self.cursor = Point::new(point.x, point.y);
                return;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                let mode = self.gallery.mode();
                self.gallery.handle_event(
                    &Event::MouseDown {
                        point: self.cursor,
                        button: MouseButton::Left,
                        mods: Default::default(),
                        count: 1,
                    },
                    self.size(),
                    self.scroll,
                );
                if mode != self.gallery.mode() {
                    self.scroll = 0.0;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.scroll -= match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 48.0,
                    MouseScrollDelta::PixelDelta(delta) => {
                        delta.y as f32 / window.scale_factor() as f32
                    }
                };
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Named(NamedKey::Tab) => {
                        self.gallery.set_mode(match self.gallery.mode() {
                            GalleryMode::Interactive => GalleryMode::AllStates,
                            GalleryMode::AllStates => GalleryMode::Interactive,
                        });
                        self.scroll = 0.0;
                    }
                    Key::Named(NamedKey::PageDown) => self.scroll += self.size().height * 0.8,
                    Key::Named(NamedKey::PageUp) => self.scroll -= self.size().height * 0.8,
                    Key::Named(NamedKey::Home) => self.scroll = 0.0,
                    Key::Named(NamedKey::End) => self.scroll = f32::MAX,
                    _ => return,
                }
            }
            _ => return,
        }
        self.clamp_scroll();
        window.request_redraw();
    }
}
