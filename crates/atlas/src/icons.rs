//! atlas icons: a tiny software rasterizer that draws the toolbar's icons into
//! white-on-transparent bitmaps once at startup, registered as Aurora images.
//!
//! Each icon is a boolean coverage predicate in a normalized `[0, 1]` square
//! (union of simple shapes - capsules, triangles, polygons, arcs, rects). The
//! rasterizer supersamples it into an alpha mask; the RGB is white, so the
//! image pipeline's tint can recolor it later. No new GPU pipeline, no bundled
//! font or art assets - the "custom Aurora icon" path from the ideatank.
//!
//! # Adding an icon
//!
//! 1. Add a variant to [`Icon`].
//! 2. Write its predicate (a `fn(f32, f32) -> bool` over the `[0, 1]` square,
//!    composed from the shape helpers below).
//! 3. Add the `(variant, predicate)` row to [`SPECS`].
//!
//! Then draw it with `icons.get(Icon::YourVariant)`. To see how it looks, run
//! the preview which renders every icon to a PNG:
//!
//! ```text
//! cargo test -p atlas icons::tests::write_preview -- --ignored --nocapture
//! ```

use std::collections::HashMap;

use aurora::ImageHandle;
use aurora_wgpu::Renderer as AuroraRenderer;

/// The rendered pixel size of each icon (square).
const SIZE: usize = 24;
/// Supersampling factor per axis for the coverage mask (anti-aliasing).
const SS: usize = 4;

/// A named icon. Add a variant here (and a row in [`SPECS`]) to add an icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Add,
    Save,
    Load,
    Select,
    Move,
    Rotate,
    Scale,
}

/// An icon's coverage predicate over the normalized `[0, 1]` square.
type Predicate = fn(f32, f32) -> bool;

/// Every icon paired with the predicate that draws it. This table IS the icon
/// set - adding a row (plus the [`Icon`] variant) is the whole change.
const SPECS: &[(Icon, Predicate)] = &[
    (Icon::Add, icon_add),
    (Icon::Save, icon_save),
    (Icon::Load, icon_load),
    (Icon::Select, icon_select),
    (Icon::Move, icon_move),
    (Icon::Rotate, icon_rotate),
    (Icon::Scale, icon_scale),
];

/// The registered icon images, looked up by [`Icon`].
pub struct Icons {
    handles: HashMap<Icon, ImageHandle>,
}

impl Icons {
    /// Rasterize and register every icon in [`SPECS`] on the GUI renderer.
    pub fn build(gui: &mut AuroraRenderer) -> Self {
        let handles = SPECS
            .iter()
            .map(|&(icon, predicate)| {
                let rgba = rasterize(predicate);
                (
                    icon,
                    gui.register_image_rgba(&rgba, SIZE as u32, SIZE as u32),
                )
            })
            .collect();
        Self { handles }
    }

    /// The registered handle for `icon` (every [`SPECS`] icon is registered).
    pub fn get(&self, icon: Icon) -> ImageHandle {
        self.handles[&icon]
    }
}

/// Supersample `inside` into a white RGBA8 mask (alpha = coverage).
fn rasterize(inside: Predicate) -> Vec<u8> {
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for py in 0..SIZE {
        for px in 0..SIZE {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / SIZE as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / SIZE as f32;
                    if inside(x, y) {
                        hits += 1;
                    }
                }
            }
            let alpha = (hits as f32 / (SS * SS) as f32 * 255.0).round() as u8;
            let i = (py * SIZE + px) * 4;
            rgba[i] = 255;
            rgba[i + 1] = 255;
            rgba[i + 2] = 255;
            rgba[i + 3] = alpha;
        }
    }
    rgba
}

// --- shape predicates, all in the normalized [0, 1] square ---

/// A thick line segment (a capsule of width `w`).
fn line(x: f32, y: f32, a: (f32, f32), b: (f32, f32), w: f32) -> bool {
    let (ax, ay) = a;
    let (bx, by) = b;
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (x - (ax + t * dx)).hypot(y - (ay + t * dy)) <= w * 0.5
}

/// A filled triangle (barycentric sign test).
fn tri(x: f32, y: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let sign = |p: (f32, f32), q: (f32, f32), r: (f32, f32)| {
        (p.0 - r.0) * (q.1 - r.1) - (q.0 - r.0) * (p.1 - r.1)
    };
    let d1 = sign((x, y), a, b);
    let d2 = sign((x, y), b, c);
    let d3 = sign((x, y), c, a);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// A filled simple polygon (even-odd ray cast).
fn poly(x: f32, y: f32, pts: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A ring segment: within the annulus `[rin, rout]` and within the angle span
/// `[a0, a1]` (radians, measured with atan2, y-down so angles grow clockwise).
fn arc(x: f32, y: f32, rin: f32, rout: f32, a0: f32, a1: f32) -> bool {
    let (dx, dy) = (x - 0.5, y - 0.5);
    let d = dx.hypot(dy);
    if d < rin || d > rout {
        return false;
    }
    let mut a = dy.atan2(dx);
    if a < 0.0 {
        a += std::f32::consts::TAU;
    }
    a >= a0 && a <= a1
}

/// A rectangle `[x0, x1] x [y0, y1]`.
fn rect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

/// A sharp arrowhead: a triangle with its `tip` pointing along `dir` (any
/// non-zero vector; normalized here), `len` long and `2*hw` wide at the base.
fn arrowhead(x: f32, y: f32, tip: (f32, f32), dir: (f32, f32), len: f32, hw: f32) -> bool {
    let inv = 1.0 / dir.0.hypot(dir.1).max(1.0e-6);
    let (dx, dy) = (dir.0 * inv, dir.1 * inv);
    let base = (tip.0 - dx * len, tip.1 - dy * len);
    let (px, py) = (-dy * hw, dx * hw);
    tri(
        x,
        y,
        tip,
        (base.0 + px, base.1 + py),
        (base.0 - px, base.1 - py),
    )
}

// --- the icons ---
//
// A common thin stroke keeps the set consistent (the Godot-toolbar look).

/// The shared line/arrow stroke width.
const STROKE: f32 = 0.07;

/// A plus.
fn icon_add(x: f32, y: f32) -> bool {
    line(x, y, (0.5, 0.18), (0.5, 0.82), 0.13) || line(x, y, (0.2, 0.5), (0.8, 0.5), 0.13)
}

/// A down arrow onto a baseline (save/export to disk).
fn icon_save(x: f32, y: f32) -> bool {
    line(x, y, (0.5, 0.14), (0.5, 0.5), STROKE)
        || arrowhead(x, y, (0.5, 0.66), (0.0, 1.0), 0.22, 0.14)
        || line(x, y, (0.22, 0.84), (0.78, 0.84), STROKE)
}

/// A folder (open/load): a body rectangle with a tab.
fn icon_load(x: f32, y: f32) -> bool {
    rect(x, y, 0.16, 0.26, 0.46, 0.4) || rect(x, y, 0.16, 0.34, 0.84, 0.78)
}

/// The classic arrow cursor (select).
fn icon_select(x: f32, y: f32) -> bool {
    const PTS: [(f32, f32); 7] = [
        (0.26, 0.14),
        (0.26, 0.8),
        (0.43, 0.63),
        (0.54, 0.86),
        (0.64, 0.81),
        (0.53, 0.59),
        (0.74, 0.56),
    ];
    poly(x, y, &PTS)
}

/// Four thin arrows radiating from the center (move), sharp heads at the edges.
fn icon_move(x: f32, y: f32) -> bool {
    line(x, y, (0.5, 0.2), (0.5, 0.8), STROKE)
        || line(x, y, (0.2, 0.5), (0.8, 0.5), STROKE)
        || arrowhead(x, y, (0.5, 0.09), (0.0, -1.0), 0.16, 0.11)
        || arrowhead(x, y, (0.5, 0.91), (0.0, 1.0), 0.16, 0.11)
        || arrowhead(x, y, (0.09, 0.5), (-1.0, 0.0), 0.16, 0.11)
        || arrowhead(x, y, (0.91, 0.5), (1.0, 0.0), 0.16, 0.11)
}

/// A circular arrow (rotate): a thin ~290-degree ring with a sharp arrowhead
/// on the upper end, pointing along the ring's tangent (clockwise).
fn icon_rotate(x: f32, y: f32) -> bool {
    use std::f32::consts::PI;
    // Ring open at the top; the upper end sits near (0.40, 0.20).
    arc(x, y, 0.28, 0.35, 0.1 * PI, 1.4 * PI)
        || arrowhead(x, y, (0.52, 0.16), (0.95, -0.31), 0.17, 0.1)
}

/// A diagonal double-headed arrow (scale/resize), thin with sharp heads.
fn icon_scale(x: f32, y: f32) -> bool {
    line(x, y, (0.26, 0.74), (0.74, 0.26), STROKE)
        || arrowhead(x, y, (0.16, 0.84), (-1.0, 1.0), 0.22, 0.12)
        || arrowhead(x, y, (0.84, 0.16), (1.0, -1.0), 0.22, 0.12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_covers_some_but_not_all_pixels() {
        // A sanity check that each predicate draws something recognizable (a
        // non-trivial coverage fraction), so a broken predicate is caught.
        for &(icon, predicate) in SPECS {
            let rgba = rasterize(predicate);
            let covered = rgba.iter().skip(3).step_by(4).filter(|&&a| a > 0).count();
            let total = SIZE * SIZE;
            assert!(
                covered > total / 40 && covered < total * 9 / 10,
                "{icon:?} coverage {covered}/{total} is out of range"
            );
        }
    }

    /// Render every icon into one PNG (white on a dark strip, scaled up) so a
    /// new or tweaked icon can be eyeballed. Ignored by default; run with
    /// `cargo test -p atlas icons::tests::write_preview -- --ignored --nocapture`
    /// then open the printed path.
    #[test]
    #[ignore = "writes a preview PNG; run with --ignored to regenerate"]
    fn write_preview() {
        const SCALE: usize = 8;
        let cols = SPECS.len();
        let (w, h) = (cols * SIZE * SCALE, SIZE * SCALE);
        let mut img = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([40, 42, 52]));
        for (col, &(_, predicate)) in SPECS.iter().enumerate() {
            let mask = rasterize(predicate);
            for py in 0..SIZE {
                for px in 0..SIZE {
                    let a = mask[(py * SIZE + px) * 4 + 3] as u32;
                    let mix = |bg: u32| ((bg * (255 - a) + 255 * a) / 255) as u8;
                    let color = image::Rgb([mix(40), mix(42), mix(52)]);
                    for yy in 0..SCALE {
                        for xx in 0..SCALE {
                            let x = (col * SIZE + px) * SCALE + xx;
                            let y = py * SCALE + yy;
                            img.put_pixel(x as u32, y as u32, color);
                        }
                    }
                }
            }
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/icons_preview.png");
        img.save(path).expect("write preview");
        println!("icon preview written to {path}");
    }
}
