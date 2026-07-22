//! orbit-renderer color: an RGBA color used for sprite tints and clear values.

/// An RGBA color with components in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// Opaque black.
    pub const BLACK: Color = Color::new(0.0, 0.0, 0.0, 1.0);
    /// Opaque white - a tint of white draws a texture unchanged.
    pub const WHITE: Color = Color::new(1.0, 1.0, 1.0, 1.0);
    /// Fully transparent.
    pub const TRANSPARENT: Color = Color::new(0.0, 0.0, 0.0, 0.0);

    /// Create a color from its components.
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub(crate) fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub(crate) fn to_wgpu(self) -> wgpu::Color {
        wgpu::Color {
            r: self.r as f64,
            g: self.g as f64,
            b: self.b as f64,
            a: self.a as f64,
        }
    }
}
