use std::sync::{Arc, Mutex};

use skia_safe::{Canvas, Rect, Size};

use crate::{
    arena::Arena, constraints::Constraints, store::Store, thunk_ext::ThunkExt, ui::UiCtx, Thunk,
    View,
};

pub struct ImageView {
    bytes: Arc<[u8]>,
    intrinsic: Size,

    decoded: Mutex<Option<skia_safe::Image>>,
}

impl Clone for ImageView {
    fn clone(&self) -> Self {
        Self {
            bytes: self.bytes.clone(),
            intrinsic: self.intrinsic,
            decoded: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for ImageView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageView")
            .field("bytes", &self.bytes.len())
            .field("intrinsic", &self.intrinsic)
            .finish()
    }
}

pub enum ImageCommand {}

impl ImageView {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Option<Self> {
        let bytes = bytes.into();
        let image = decode(&bytes)?;
        let intrinsic = Size::new(image.width() as f32, image.height() as f32);
        if intrinsic.width < 1.0 || intrinsic.height < 1.0 {
            return None;
        }
        Some(Self {
            bytes,
            intrinsic,
            decoded: Mutex::new(Some(image)),
        })
    }

    pub fn intrinsic(&self) -> Size {
        self.intrinsic
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn scaled(&self, width: f32) -> Size {
        let scale = (width.max(1.0) / self.intrinsic.width.max(1.0)).min(1.0);
        Size::new(
            (self.intrinsic.width * scale).max(1.0),
            (self.intrinsic.height * scale).max(1.0),
        )
    }

    pub fn paint(&self, canvas: &Canvas, rect: Rect) {
        let mut cell = self.decoded.lock().expect("image decode cell");
        if cell.is_none() {
            *cell = decode(&self.bytes);
        }
        let Some(image) = cell.as_ref() else {
            return;
        };
        let paint = skia_safe::Paint::default();
        canvas.draw_image_rect_with_sampling_options(
            image,
            None,
            rect,
            skia_safe::SamplingOptions::from(skia_safe::FilterMode::Linear),
            &paint,
        );
    }
}

fn decode(bytes: &[u8]) -> Option<skia_safe::Image> {
    skia_safe::images::deferred_from_encoded_data(skia_safe::Data::new_copy(bytes), None)
}

impl View for ImageView {
    type Command = ImageCommand;

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

    fn png() -> Vec<u8> {
        let surface = skia_safe::surfaces::raster_n32_premul((2, 1)).expect("surface");
        let mut surface = surface;
        surface.canvas().clear(skia_safe::Color::RED);
        let image = surface.image_snapshot();
        image
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("encoded")
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn encoded_bytes_decode_to_their_intrinsic_size() {
        let view = ImageView::new(png()).expect("decodes");
        assert_eq!(view.intrinsic(), Size::new(2.0, 1.0));
    }

    #[test]
    fn junk_bytes_decode_to_nothing() {
        assert!(ImageView::new(b"not an image".to_vec()).is_none());
    }

    #[test]
    fn a_wide_image_scales_down_and_a_narrow_one_does_not() {
        let view = ImageView::new(png()).expect("decodes");
        assert_eq!(view.scaled(1.0), Size::new(1.0, 1.0), "halved");
        assert_eq!(view.scaled(100.0), Size::new(2.0, 1.0), "never upscaled");
    }

    #[test]
    fn the_view_paints_its_pixels() {
        let view = ImageView::new(png()).expect("decodes");
        let mut surface = skia_safe::surfaces::raster_n32_premul((2, 1)).expect("surface");
        surface.canvas().clear(skia_safe::Color::WHITE);
        view.paint(surface.canvas(), Rect::from_xywh(0.0, 0.0, 2.0, 1.0));
        let image = surface.image_snapshot();
        let mut pixels = vec![0u8; 2 * 4];
        let info = skia_safe::ImageInfo::new(
            (2, 1),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Premul,
            None,
        );
        assert!(image.read_pixels(&info, &mut pixels, 8, (0, 0), skia_safe::image::CachingHint::Allow));
        assert_eq!(&pixels[..3], &[255, 0, 0], "the red pixel painted");
    }
}
