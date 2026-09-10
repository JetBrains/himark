use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use imba::{
    arena::Arena, constraints::Constraints, store::Store, thunk_ext::ThunkExt, Thunk, View,
};
use skia_safe::{Canvas, Font, Paint, Rect};

const FPS_WINDOW: Duration = Duration::from_millis(500);
const RENDER_WINDOW: usize = 100;

pub struct Stats {
    font: Font,
    fps_window_started: Instant,
    fps_frames: u32,
    fps: f32,
    render_samples: [u64; RENDER_WINDOW],
    render_sample_index: usize,
    latency_ns: Arc<AtomicU64>,
    latency_event_started_at: Option<f64>,

    reconcile_streak: u32,
}

pub enum StatsCommand {}

impl Stats {
    pub(crate) fn new(font: Font) -> Self {
        Self {
            font,
            fps_window_started: Instant::now(),
            fps_frames: 0,
            fps: 0.0,
            render_samples: [0; RENDER_WINDOW],
            render_sample_index: 0,
            latency_ns: Arc::new(AtomicU64::new(0)),
            latency_event_started_at: None,
            reconcile_streak: 0,
        }
    }

    pub(crate) fn set_font(&mut self, font: Font) {
        self.font = font;
    }

    pub fn latency_ns(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.latency_ns)
    }

    pub fn latency_event_start(&self) -> Option<f64> {
        self.latency_event_started_at
    }

    pub fn clear_latency_event_start(&mut self) {
        self.latency_event_started_at = None;
    }

    pub(crate) fn mark_latency_event_start(&mut self, event_started_at: f64) {
        self.latency_event_started_at = Some(event_started_at);
    }

    pub(crate) fn begin_frame(&mut self) {
        self.fps_frames += 1;
        let elapsed = self.fps_window_started.elapsed();
        if elapsed >= FPS_WINDOW {
            self.fps = self.fps_frames as f32 / elapsed.as_secs_f32();
            self.fps_frames = 0;
            self.fps_window_started = Instant::now();
        }
    }

    pub(crate) fn record_reconcile(&mut self, reconciled: bool) {
        self.reconcile_streak = if reconciled {
            self.reconcile_streak + 1
        } else {
            0
        };
        if self.reconcile_streak == 240 {
            eprintln!(
                "[himark] the paint reconcile has not settled for 240 frames — \
                 two views are likely fighting over one entity's geometry"
            );
        }
    }

    pub(crate) fn record_render_sample(&mut self, duration: Duration) {
        self.render_samples[self.render_sample_index] =
            duration.as_nanos().min(u64::MAX as u128) as u64;
        self.render_sample_index = (self.render_sample_index + 1) % RENDER_WINDOW;
    }

    fn paint(&self, canvas: &Canvas, rect: Rect, chrome: &::editor::theme::StatsChrome) {
        let latency_ns = self.latency_ns.load(Ordering::Relaxed);
        let latency = match latency_ns {
            0 => "--.- ms".to_string(),
            ns => format!("{:.1} ms", ns as f32 / 1_000_000.0),
        };
        let fps = match self.fps > 0.0 {
            true => format!("{:.0} fps", self.fps),
            false => "-- fps".to_string(),
        };
        let render_ns = self.render_samples.iter().copied().max().unwrap_or(0);
        let render = match render_ns {
            0 => "draw --.- ms".to_string(),
            ns => format!("draw {:.1} ms", ns as f32 / 1_000_000.0),
        };

        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(chrome.background.0);
        let left = rect.right - chrome.right_margin - chrome.width;
        canvas.draw_round_rect(
            Rect::from_xywh(left, chrome.top, chrome.width, chrome.height),
            chrome.radius,
            chrome.radius,
            &paint,
        );

        paint.set_color(chrome.text.0);
        let baseline = chrome.top + chrome.first_baseline;
        canvas.draw_str(fps, (left + chrome.pad_x, baseline), &self.font, &paint);
        canvas.draw_str(
            latency,
            (left + chrome.pad_x, baseline + chrome.line_height),
            &self.font,
            &paint,
        );
        canvas.draw_str(
            render,
            (left + chrome.pad_x, baseline + chrome.line_height * 2.0),
            &self.font,
            &paint,
        );
    }
}

impl View for Stats {
    type Command = StatsCommand;
    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {}
    }

    fn layout<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        _ui: &'a imba::UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let theme = ::editor::env::Themes::of(store);
        imba::leaf::leaf(constraints.max.width, constraints.max.height)
            .paint_instead(move |_arena, canvas, rect| self.paint(canvas, rect, &theme.ui().stats))
    }
}
