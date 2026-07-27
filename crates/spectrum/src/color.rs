//! HSV/RGB/hex color math and the gradient bitmaps a color picker draws (an SV
//! square, a hue bar, an alpha bar), each with its current marker baked in.
//! Pure and headless; the picker uploads these RGBA buffers as images.

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
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
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

// --- bitmap generators (RGBA8, row-major) ---

/// The saturation/value square for a given `hue`: saturation left->right,
/// value bottom->top. A ring marks the current `(s, v)`.
pub fn sv_square(hue: f32, size: usize, s: f32, v: f32) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let sat = x as f32 / (size - 1) as f32;
            let val = 1.0 - y as f32 / (size - 1) as f32;
            let [r, g, b] = hsv_to_rgb(hue, sat, val);
            put(&mut px, size, x, y, [byte(r), byte(g), byte(b), 255]);
        }
    }
    let mx = (s * (size - 1) as f32) as i32;
    let my = ((1.0 - v) * (size - 1) as f32) as i32;
    draw_ring(&mut px, size, size, mx, my, 4.0);
    px
}

/// A vertical hue bar (full spectrum top->bottom), `w` by `h`, with a marker
/// at the current `hue`.
pub fn hue_bar(w: usize, h: usize, hue: f32) -> Vec<u8> {
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        let [r, g, b] = hsv_to_rgb(y as f32 / (h - 1) as f32, 1.0, 1.0);
        for x in 0..w {
            put(&mut px, w, x, y, [byte(r), byte(g), byte(b), 255]);
        }
    }
    draw_hline_marker(&mut px, w, h, (hue * (h - 1) as f32) as i32);
    px
}

/// A horizontal alpha bar for `rgb` over a checkerboard (transparent left ->
/// opaque right), `w` by `h`, with a marker at the current alpha.
pub fn alpha_bar(rgb: [f32; 3], w: usize, h: usize, alpha: f32) -> Vec<u8> {
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let a = x as f32 / (w - 1) as f32;
            // Checkerboard so transparency reads.
            let check = if (x / 6 + y / 6) % 2 == 0 { 0.55 } else { 0.4 };
            let blend = |c: f32| byte(c * a + check * (1.0 - a));
            put(
                &mut px,
                w,
                x,
                y,
                [blend(rgb[0]), blend(rgb[1]), blend(rgb[2]), 255],
            );
        }
    }
    draw_vline_marker(&mut px, w, h, (alpha * (w - 1) as f32) as i32);
    px
}

fn byte(f: f32) -> u8 {
    (f.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn put(px: &mut [u8], stride: usize, x: usize, y: usize, rgba: [u8; 4]) {
    let i = (y * stride + x) * 4;
    px[i..i + 4].copy_from_slice(&rgba);
}

/// A hollow ring marker (white core, dark edge for contrast on any color).
fn draw_ring(px: &mut [u8], w: usize, h: usize, cx: i32, cy: i32, r: f32) {
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let d = (((x - cx) * (x - cx) + (y - cy) * (y - cy)) as f32).sqrt();
            let ring = (d - r).abs();
            if ring <= 1.6 {
                let shade = if ring <= 0.8 { 255 } else { 20 };
                put(px, w, x as usize, y as usize, [shade, shade, shade, 255]);
            }
        }
    }
}

/// A short marker line across the left/right edges of a vertical bar at row `y`.
fn draw_hline_marker(px: &mut [u8], w: usize, h: usize, my: i32) {
    for y in (my - 1).max(0)..=(my + 1).min(h as i32 - 1) {
        let shade = if y == my { 255 } else { 20 };
        for x in 0..w {
            put(px, w, x, y as usize, [shade, shade, shade, 255]);
        }
    }
}

/// A marker line down a horizontal bar at column `x`.
fn draw_vline_marker(px: &mut [u8], w: usize, h: usize, mx: i32) {
    for x in (mx - 1).max(0)..=(mx + 1).min(w as i32 - 1) {
        let shade = if x == mx { 255 } else { 20 };
        for y in 0..h {
            put(px, w, x as usize, y, [shade, shade, shade, 255]);
        }
    }
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

    #[test]
    fn generators_produce_right_sized_opaque_buffers() {
        assert_eq!(sv_square(0.3, 32, 0.5, 0.5).len(), 32 * 32 * 4);
        assert_eq!(hue_bar(8, 40, 0.5).len(), 8 * 40 * 4);
        assert_eq!(alpha_bar([1.0, 0.0, 0.0], 40, 8, 0.5).len(), 40 * 8 * 4);
    }
}
