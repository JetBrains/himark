// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! UI component gallery plugin and its independent screenshot renderer.
mod panel;
mod render;
mod view;

pub use panel::{GalleryPanel, OpenGallery};
pub use render::{Gallery, SCREENSHOT_WIDTH};
pub use view::{GalleryCommand, GalleryMode, GalleryView};

#[cfg(test)]
mod tests;
