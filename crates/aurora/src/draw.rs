//! aurora draw list: the high-level commands Aurora emits for a backend to render (ADR 0015).

use cosmic_text::CacheKey;

use crate::color::Color;
use crate::rect::Rect;

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
    /// Draw a run of glyphs in one color (a shaped line of text).
    Text { glyphs: Vec<Glyph>, color: Color },
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
