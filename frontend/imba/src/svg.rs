use std::sync::{Arc, Mutex, OnceLock};

use skia_safe::{Canvas, Rect, Size};

use crate::{
    arena::Arena, constraints::Constraints, store::Store, thunk_ext::ThunkExt, ui::UiCtx, Thunk,
    View,
};

fn fontdb() -> &'static Arc<resvg::usvg::fontdb::Database> {
    static FONTDB: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONTDB.get_or_init(|| {
        let mut database = resvg::usvg::fontdb::Database::new();
        database.load_system_fonts();
        Arc::new(database)
    })
}

pub struct SvgView {
    svg: Arc<str>,
    intrinsic: Size,

    scale_limit: f32,

    raster: Mutex<Option<(f32, skia_safe::Image)>>,
}

impl Clone for SvgView {
    fn clone(&self) -> Self {
        Self {
            svg: self.svg.clone(),
            intrinsic: self.intrinsic,
            scale_limit: self.scale_limit,
            raster: Mutex::new(None),
        }
    }
}

pub enum SvgCommand {}

impl SvgView {
    pub fn new(svg: impl Into<Arc<str>>, intrinsic: Size) -> Self {
        Self {
            svg: svg.into(),
            intrinsic,
            scale_limit: 1.0,
            raster: Mutex::new(None),
        }
    }

    pub fn with_scale_limit(mut self, limit: f32) -> Self {
        self.scale_limit = limit.max(0.1);
        self
    }

    pub fn svg(&self) -> &str {
        &self.svg
    }

    pub fn intrinsic(&self) -> Size {
        self.intrinsic
    }

    pub fn scaled(&self, width: f32) -> Size {
        let scale = self.scale(width);
        Size::new(self.intrinsic.width * scale, self.intrinsic.height * scale)
    }

    fn scale(&self, width: f32) -> f32 {
        (width.max(1.0) / self.intrinsic.width.max(1.0)).min(self.scale_limit)
    }

    pub fn paint(&self, canvas: &Canvas, rect: Rect) {
        let device_scale = {
            let matrix = canvas.local_to_device_as_3x3();
            matrix.scale_x().abs().max(matrix.scale_y().abs()).max(0.5)
        };

        let draw_scale = (rect.width() / self.intrinsic.width.max(1.0)).min(self.scale_limit);
        let raster_scale = draw_scale * device_scale;

        let mut cell = self.raster.lock().expect("svg raster cell");
        let stale = match cell.as_ref() {
            Some((cached, _)) => (cached - raster_scale).abs() > 0.01,
            None => true,
        };
        if stale {
            *cell = self
                .rasterize(raster_scale)
                .map(|image| (raster_scale, image));
        }
        let Some((_, image)) = cell.as_ref() else {
            return;
        };

        let paint = skia_safe::Paint::default();
        canvas.save();
        canvas.translate((rect.left, rect.top));
        let inverse = draw_scale / raster_scale;
        canvas.scale((inverse, inverse));
        canvas.draw_image_with_sampling_options(
            image,
            (0.0, 0.0),
            skia_safe::SamplingOptions::from(skia_safe::FilterMode::Linear),
            Some(&paint),
        );
        canvas.restore();
    }

    fn rasterize(&self, scale: f32) -> Option<skia_safe::Image> {
        let options = resvg::usvg::Options {
            fontdb: Arc::clone(fontdb()),
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_str(&self.svg, &options).ok()?;
        let width = (self.intrinsic.width * scale).ceil().max(1.0) as u32;
        let height = (self.intrinsic.height * scale).ceil().max(1.0) as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;

        let size = tree.size();
        let transform = resvg::tiny_skia::Transform::from_scale(
            width as f32 / size.width().max(1.0),
            height as f32 / size.height().max(1.0),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());

        let info = skia_safe::ImageInfo::new(
            (width as i32, height as i32),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Premul,
            None,
        );
        skia_safe::images::raster_from_data(
            &info,
            skia_safe::Data::new_copy(pixmap.data()),
            width as usize * 4,
        )
    }

    pub fn intrinsic_size(svg: &str) -> Option<Size> {
        let root_end = svg.find('>')?;
        let root = &svg[..root_end];
        let attr = |name: &str| -> Option<String> {
            let at = root.find(&format!("{name}=\""))?;
            let rest = &root[at + name.len() + 2..];
            let end = rest.find('"')?;
            Some(rest[..end].to_owned())
        };
        let absolute =
            |name: &str| -> Option<f32> { attr(name)?.trim_end_matches("px").parse().ok() };
        if let (Some(width), Some(height)) = (absolute("width"), absolute("height")) {
            return Some(Size::new(width, height));
        }
        let view_box = attr("viewBox")?;
        let mut parts = view_box.split_whitespace().skip(2);
        let width: f32 = parts.next()?.parse().ok()?;
        let height: f32 = parts.next()?.parse().ok()?;
        Some(Size::new(width, height))
    }
}

impl View for SvgView {
    type Command = SvgCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut crate::effect::Effects<'_, Self::Command>,
    ) {
        match command {}
    }

    fn layout<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let size = self.scaled(constraints.max.width);
        crate::leaf::leaf(size.width, size.height)
            .paint_instead(move |_, canvas, rect| self.paint(canvas, rect))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_styled_svg_paints_its_colors() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
            <style>.box { fill: #ff0000; }</style>
            <rect class="box" x="0" y="0" width="40" height="20"/>
        </svg>"##;
        let view = SvgView::new(svg, Size::new(40.0, 20.0));
        let mut surface = skia_safe::surfaces::raster_n32_premul((40, 20)).expect("surface");
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::WHITE);
        view.paint(canvas, Rect::from_wh(40.0, 20.0));
        let image = surface.image_snapshot();
        let pixels = image.peek_pixels().expect("pixels");

        let color: skia_safe::Color = pixels.get_color((20, 10));
        let (r, g, b) = (color.r(), color.g(), color.b());
        assert!(
            r > 200 && g < 60 && b < 60,
            "the class-styled rect paints red: ({r},{g},{b})"
        );
    }

    #[test]
    fn intrinsic_size_reads_absolute_and_viewbox_roots() {
        assert_eq!(
            SvgView::intrinsic_size(r#"<svg width="120" height="60">"#),
            Some(Size::new(120.0, 60.0))
        );

        assert_eq!(
            SvgView::intrinsic_size(r#"<svg width="100%" viewBox="0 0 115.19 190">"#),
            Some(Size::new(115.19, 190.0))
        );
        assert_eq!(SvgView::intrinsic_size(r#"<svg width="100%">"#), None);
    }
}
