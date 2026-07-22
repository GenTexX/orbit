//! photon color: an RGBA color used for sprite tints and clear values.

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

    /// Create an opaque color from HSV. Hue wraps around; saturation and value
    /// are clamped to `0.0..=1.0` by the caller. `hsv(0, 1, 1)` is red,
    /// `hsv(1/3, 1, 1)` is green, `hsv(2/3, 1, 1)` is blue.
    pub fn hsv(hue: f32, saturation: f32, value: f32) -> Self {
        let hp = (hue.fract() + 1.0).fract() * 6.0;
        let c = value * saturation;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let m = value - c;
        let (r, g, b) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        Self::new(r + m, g + m, b + m, 1.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rgb(color: Color, r: f32, g: f32, b: f32) {
        const EPS: f32 = 1e-4;
        assert!(
            (color.r - r).abs() < EPS
                && (color.g - g).abs() < EPS
                && (color.b - b).abs() < EPS
                && (color.a - 1.0).abs() < EPS,
            "got {color:?}, expected ({r}, {g}, {b}, 1.0)"
        );
    }

    #[test]
    fn hsv_primaries_and_grays() {
        assert_rgb(Color::hsv(0.0, 1.0, 1.0), 1.0, 0.0, 0.0);
        assert_rgb(Color::hsv(1.0 / 3.0, 1.0, 1.0), 0.0, 1.0, 0.0);
        assert_rgb(Color::hsv(2.0 / 3.0, 1.0, 1.0), 0.0, 0.0, 1.0);
        assert_rgb(Color::hsv(0.5, 1.0, 1.0), 0.0, 1.0, 1.0); // cyan
        assert_rgb(Color::hsv(0.0, 0.0, 1.0), 1.0, 1.0, 1.0); // white
        assert_rgb(Color::hsv(0.0, 0.0, 0.5), 0.5, 0.5, 0.5); // gray
    }

    #[test]
    fn hsv_hue_wraps() {
        assert_rgb(Color::hsv(1.0, 1.0, 1.0), 1.0, 0.0, 0.0); // wraps to 0.0 = red
    }
}
