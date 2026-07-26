//! aurora color: an RGBA color for widget fills and clear values.
//!
//! Aurora keeps its own `Color` (a trivial four-float struct) rather than borrow
//! photon's, so the GUI stays independent of the engine and reusable elsewhere.

/// An RGBA color with components in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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

    /// Mix each channel `t` of the way toward white (`t` in `0.0..=1.0`),
    /// preserving alpha. Used for hover highlights on controls.
    pub fn lighten(self, t: f32) -> Self {
        Self::rgba(
            self.r + (1.0 - self.r) * t,
            self.g + (1.0 - self.g) * t,
            self.b + (1.0 - self.b) * t,
            self.a,
        )
    }

    /// Scale each channel toward black by `t` (`t` in `0.0..=1.0`), preserving
    /// alpha. Used for the pressed state on controls.
    pub fn darken(self, t: f32) -> Self {
        Self::rgba(
            self.r * (1.0 - t),
            self.g * (1.0 - t),
            self.b * (1.0 - t),
            self.a,
        )
    }

    /// Scale the alpha by `t` (`t` in `0.0..=1.0`), keeping the color. Used to
    /// fade a disabled widget's fills and text toward the background.
    pub fn fade(self, t: f32) -> Self {
        Self::rgba(self.r, self.g, self.b, self.a * t)
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}
