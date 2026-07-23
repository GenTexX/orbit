//! aurora draw list: the high-level commands Aurora emits for a backend to render (ADR 0015).

use crate::color::Color;
use crate::rect::Rect;

/// One drawing command. A backend (e.g. aurora-wgpu) turns a list of these into
/// GPU work; Aurora itself never names a GPU type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawCommand {
    /// Fill an axis-aligned rectangle with a solid color.
    FillRect { rect: Rect, color: Color },
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
