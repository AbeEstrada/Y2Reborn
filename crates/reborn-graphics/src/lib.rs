mod native;
pub use native::{Atlases, Renderer};
use serde::{Deserialize, Serialize};

/// Which texture a quad samples. Glyph pages, the shape/icon mask page and the
/// thumbnail atlas are addressed with `Quad::src`, in texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Tex {
    /// A flat colour.
    #[default]
    Solid,
    /// Glyph page 0 Regular, 1 SemiBold or 2 the fallback page; coverage masks
    /// tinted by `color`.
    Font(u8),
    /// Shapes, gradients and icons; coverage masks tinted by `color`.
    Mask,
    /// The current track's artwork (160x160).
    Art,
    /// The artwork of the album, artist or song being browsed (160x160).
    CollectionArt,
    /// A library thumbnail (96x96) identified by an opaque key; the renderer
    /// maps it to an atlas cell and skips it until its pixels are resident.
    Thumb(u64),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: u32,
    #[serde(default)]
    pub tex: Tex,
    /// Source rectangle `[x, y, w, h]` in texels (glyph, mask textures only).
    #[serde(default)]
    pub src: [u16; 4],
    /// Presentation metadata used by deterministic UI validation. Native
    /// rendering intentionally ignores this field.
    pub focus_target: bool,
}
impl Quad {
    pub fn rect(x: f32, y: f32, w: f32, h: f32, color: u32) -> Self {
        Self {
            x,
            y,
            w,
            h,
            color,
            tex: Tex::Solid,
            src: [0; 4],
            focus_target: false,
        }
    }
    /// A tinted region of a coverage-mask texture.
    pub fn masked(tex: Tex, src: [u16; 4], x: f32, y: f32, w: f32, h: f32, color: u32) -> Self {
        Self {
            tex,
            src,
            ..Self::rect(x, y, w, h, color)
        }
    }
}

/// Texture geometry shared with `native/graphics.c`.
pub const FONT_PAGE_WIDTH: usize = 1024;
pub const THUMB_SIZE: usize = 96;
/// A thumbnail cell is the image with a replicated one-pixel gutter.
pub const THUMB_CELL: usize = THUMB_SIZE + 2;
pub const THUMB_COLUMNS: usize = 1024 / THUMB_CELL;
pub const THUMB_ROWS: usize = 512 / THUMB_CELL;
pub const THUMB_SLOTS: usize = THUMB_COLUMNS * THUMB_ROWS;
