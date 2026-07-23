//! aurora color: an RGBA color for widget fills and clear values.
//!
//! Aurora keeps its own `Color` (a trivial four-float struct) rather than borrow
//! photon's, so the GUI stays independent of the engine and reusable elsewhere.

/// An RGBA color with components in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// Fully transparent (and the default): draws nothing.
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);
    /// Opaque black.
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);
    /// Opaque white.
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);

    /// A color from all four components.
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// An opaque color (alpha 1).
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::rgba(r, g, b, 1.0)
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}
