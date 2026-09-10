use std::sync::OnceLock;

use skia_safe::textlayout::{FontCollection, TypefaceFontProvider};
use skia_safe::{FontMgr, Typeface};

#[cfg(not(target_os = "emscripten"))]
static NOTO_SANS: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");

static TYPEFACE: OnceLock<Typeface> = OnceLock::new();

#[cfg(not(target_os = "emscripten"))]
pub const FAMILY: &str = "Noto Sans";

#[cfg(target_os = "emscripten")]
pub const FAMILY: &str = "JetBrains Mono";

#[cfg(target_os = "emscripten")]
pub fn install(typeface: Typeface) {
    assert!(
        TYPEFACE.set(typeface).is_ok(),
        "the web typeface was installed more than once"
    );
}

pub fn typeface() -> Typeface {
    #[cfg(not(target_os = "emscripten"))]
    {
        TYPEFACE
            .get_or_init(|| {
                FontMgr::new()
                    .new_from_data(NOTO_SANS, None)
                    .expect("embedded Noto Sans loads")
            })
            .clone()
    }
    #[cfg(target_os = "emscripten")]
    {
        TYPEFACE
            .get()
            .expect("the web font must load before the application starts")
            .clone()
    }
}

pub fn collection() -> FontCollection {
    let mut provider = TypefaceFontProvider::new();
    provider.register_typeface(typeface(), Some(FAMILY));
    let mut collection = FontCollection::new();
    collection.set_default_font_manager(FontMgr::from(provider), Some(FAMILY));
    collection
}

pub fn source() -> crate::FontSource {
    std::sync::Arc::new(|| {
        thread_local! {
            static COLLECTION: FontCollection = collection();
        }
        COLLECTION.with(Clone::clone)
    })
}
