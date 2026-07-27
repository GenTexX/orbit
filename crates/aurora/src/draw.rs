//! aurora draw list: the high-level commands Aurora emits for a backend to render (ADR 0015).

use cosmic_text::CacheKey;
use slotmap::new_key_type;

use crate::color::Color;
use crate::rect::Rect;

new_key_type! {
    /// An opaque reference to a texture a backend has registered (ADR 0018),
    /// e.g. the engine's rendered scene. Aurora carries only this handle in a
    /// draw command; it never names the underlying GPU texture type, so a
    /// backend can swap without this crate changing.
    pub struct ImageHandle;
}

/// A single positioned glyph. `cache_key` identifies the glyph (font, id, size,
/// subpixel offset) for a backend to rasterize and cache; `x`/`y` are its pen
/// position on screen in pixels (the baseline origin). Carrying a cosmic-text
/// `CacheKey` couples the draw list to cosmic-text's text model, as accepted in
/// ADR 0013.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub cache_key: CacheKey,
    pub x: f32,
    pub y: f32,
}

/// One drawing command. A backend (e.g. aurora-wgpu) turns a list of these into
/// GPU work; Aurora itself never names a GPU type.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawCommand {
    /// Fill an axis-aligned rectangle with a solid color.
    FillRect { rect: Rect, color: Color },
    /// Fill a rectangle with rounded corners and/or a border. `radius` is the
    /// per-corner radius in px, `[top_left, top_right, bottom_right, bottom_left]`
    /// (0 = square); `border_width` the border thickness in px (0 = no border),
    /// painted in `border_color`. `border_sides` selects which edges the border
    /// is drawn on, `[top, right, bottom, left]` - an open edge (e.g. the bottom
    /// of a folder-style tab) leaves the fill reaching that edge with no stroke.
    /// Corners are anti-aliased.
    RoundedRect {
        rect: Rect,
        color: Color,
        radius: [f32; 4],
        border_width: f32,
        border_color: Color,
        border_sides: [bool; 4],
    },
    /// Draw a run of glyphs in one color (a shaped line of text).
    Text { glyphs: Vec<Glyph>, color: Color },
    /// Draw a registered texture stretched to fill a rectangle (e.g. the
    /// engine's viewport). `handle` is resolved by the backend, not Aurora.
    /// `tint` multiplies the sampled texture - `Color::WHITE` is a straight
    /// passthrough; a color recolors a white icon (e.g. an icon on hover).
    Image {
        rect: Rect,
        handle: ImageHandle,
        tint: Color,
    },
    /// Push a clip rectangle. Following draws are clipped to the intersection of
    /// all active clips until the matching `PopClip`.
    PushClip { rect: Rect },
    /// Pop the most recently pushed clip.
    PopClip,
}

/// An ordered list of drawing commands for one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawList {
    pub commands: Vec<DrawCommand>,
}
