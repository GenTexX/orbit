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

// --- HSV and hex ---
//
// A picker edits in HSV (a hue ring plus a saturation/value square is how people
// actually choose a color), while everything else here is RGBA; `Hsva` is the
// conversion between them, and hex is how a color is typed and copied.

/// A color in HSVA, each component in `0.0..=1.0` (hue wraps).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsva {
    pub h: f32,
    pub s: f32,
    pub v: f32,
    pub a: f32,
}

impl Hsva {
    /// Convert from straight RGBA (each `0.0..=1.0`). Hue is left unchanged for
    /// greys (where it is undefined), so editing stays stable.
    pub fn from_rgba(rgba: [f32; 4], keep_hue: f32) -> Self {
        let [r, g, b, a] = rgba;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let v = max;
        let s = if max <= 0.0 { 0.0 } else { delta / max };
        let h = if delta <= 0.0 {
            keep_hue
        } else if max == r {
            (((g - b) / delta) % 6.0) / 6.0
        } else if max == g {
            ((b - r) / delta + 2.0) / 6.0
        } else {
            ((r - g) / delta + 4.0) / 6.0
        };
        let h = h.rem_euclid(1.0);
        Self { h, s, v, a }
    }

    /// Convert to straight RGBA (each `0.0..=1.0`).
    pub fn to_rgba(self) -> [f32; 4] {
        let [r, g, b] = hsv_to_rgb(self.h, self.s, self.v);
        [r, g, b, self.a]
    }
}

/// HSV (hue wraps, s/v in `0..=1`) to RGB.
pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// Format an RGBA color as `#RRGGBBAA` (uppercase).
pub fn to_hex(rgba: [f32; 4]) -> String {
    let byte = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        byte(rgba[0]),
        byte(rgba[1]),
        byte(rgba[2]),
        byte(rgba[3])
    )
}

/// Parse `#RGB`, `#RRGGBB`, or `#RRGGBBAA` (the `#` optional) into RGBA. Alpha
/// defaults to opaque when absent.
pub fn from_hex(text: &str) -> Option<[f32; 4]> {
    let hex = text.trim().trim_start_matches('#');
    let byte = |s: &str| u8::from_str_radix(s, 16).ok().map(|b| b as f32 / 255.0);
    let (r, g, b, a) = match hex.len() {
        3 => {
            // #RGB shorthand (each nibble doubled).
            let dup = |c: char| {
                u8::from_str_radix(&format!("{c}{c}"), 16)
                    .ok()
                    .map(|b| b as f32 / 255.0)
            };
            let mut it = hex.chars();
            (dup(it.next()?)?, dup(it.next()?)?, dup(it.next()?)?, 1.0)
        }
        6 => (byte(&hex[0..2])?, byte(&hex[2..4])?, byte(&hex[4..6])?, 1.0),
        8 => (
            byte(&hex[0..2])?,
            byte(&hex[2..4])?,
            byte(&hex[4..6])?,
            byte(&hex[6..8])?,
        ),
        _ => return None,
    };
    Some([r, g, b, a])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1.0e-3)
    }

    #[test]
    fn hsv_rgb_round_trips_for_saturated_colors() {
        for rgba in [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 0.5],
            [0.0, 0.0, 1.0, 1.0],
            [0.2, 0.6, 0.4, 0.8],
        ] {
            let back = Hsva::from_rgba(rgba, 0.0).to_rgba();
            assert!(close(rgba, back), "{rgba:?} -> {back:?}");
        }
    }

    #[test]
    fn grey_keeps_the_supplied_hue() {
        // A grey has no defined hue; conversion keeps the caller's, so dragging
        // value to zero and back does not snap the hue to red.
        let hsva = Hsva::from_rgba([0.5, 0.5, 0.5, 1.0], 0.7);
        assert_eq!(hsva.h, 0.7);
        assert!(hsva.s < 1.0e-3);
    }

    #[test]
    fn hex_round_trips_and_parses_shorthand() {
        assert_eq!(to_hex([1.0, 0.0, 0.0, 1.0]), "#FF0000FF");
        assert!(close(from_hex("#FF0000FF").unwrap(), [1.0, 0.0, 0.0, 1.0]));
        assert!(close(from_hex("00ff00").unwrap(), [0.0, 1.0, 0.0, 1.0]));
        assert!(close(from_hex("#f00").unwrap(), [1.0, 0.0, 0.0, 1.0]));
        assert_eq!(from_hex("nope"), None);
    }
}
