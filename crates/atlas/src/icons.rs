//! atlas icons: a tiny software rasterizer that draws the toolbar's icons into
//! white-on-transparent bitmaps once at startup, registered as Aurora images.
//!
//! Each icon is a boolean coverage predicate in a normalized `[0, 1]` square
//! (union of simple shapes - discs, capsules, triangles, polygons, arcs). The
//! rasterizer supersamples it into an alpha mask; the RGB is white, so the
//! image pipeline's tint can recolor it later. No new GPU pipeline, no bundled
//! font or art assets - the "custom Aurora icon" path from the ideatank.

use aurora::ImageHandle;
use aurora_wgpu::Renderer as AuroraRenderer;

/// The rendered pixel size of each icon (square).
const SIZE: usize = 24;
/// Supersampling factor per axis for the coverage mask (anti-aliasing).
const SS: usize = 4;

/// The registered toolbar icons (opaque handles; cheap to copy).
#[derive(Debug, Clone, Copy)]
pub struct Icons {
    pub add: ImageHandle,
    pub save: ImageHandle,
    pub load: ImageHandle,
    pub select: ImageHandle,
    pub translate: ImageHandle,
    pub rotate: ImageHandle,
    pub scale: ImageHandle,
}

impl Icons {
    /// Rasterize and register every toolbar icon on the GUI renderer.
    pub fn build(gui: &mut AuroraRenderer) -> Self {
        let mut register = |predicate: fn(f32, f32) -> bool| {
            let rgba = rasterize(predicate);
            gui.register_image_rgba(&rgba, SIZE as u32, SIZE as u32)
        };
        Self {
            add: register(icon_add),
            save: register(icon_save),
            load: register(icon_load),
            select: register(icon_select),
            translate: register(icon_move),
            rotate: register(icon_rotate),
            scale: register(icon_scale),
        }
    }
}

/// Supersample `inside` into a white RGBA8 mask (alpha = coverage).
fn rasterize(inside: fn(f32, f32) -> bool) -> Vec<u8> {
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

// --- the icons ---

/// A plus.
fn icon_add(x: f32, y: f32) -> bool {
    line(x, y, (0.5, 0.2), (0.5, 0.8), 0.16) || line(x, y, (0.2, 0.5), (0.8, 0.5), 0.16)
}

/// A down arrow onto a baseline (save/export to disk).
fn icon_save(x: f32, y: f32) -> bool {
    line(x, y, (0.5, 0.14), (0.5, 0.52), 0.13)
        || tri(x, y, (0.32, 0.46), (0.68, 0.46), (0.5, 0.68))
        || line(x, y, (0.22, 0.84), (0.78, 0.84), 0.11)
}

/// A rectangle `[x0, x1] x [y0, y1]`.
fn rect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
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

/// Four-way arrows (move).
fn icon_move(x: f32, y: f32) -> bool {
    line(x, y, (0.5, 0.2), (0.5, 0.8), 0.1)
        || line(x, y, (0.2, 0.5), (0.8, 0.5), 0.1)
        || tri(x, y, (0.5, 0.08), (0.4, 0.24), (0.6, 0.24))
        || tri(x, y, (0.5, 0.92), (0.4, 0.76), (0.6, 0.76))
        || tri(x, y, (0.08, 0.5), (0.24, 0.4), (0.24, 0.6))
        || tri(x, y, (0.92, 0.5), (0.76, 0.4), (0.76, 0.6))
}

/// A circular arrow (rotate): a ~300-degree ring with an arrowhead at one end.
fn icon_rotate(x: f32, y: f32) -> bool {
    use std::f32::consts::PI;
    // Ring open near the top (a gap around angle -PI/2, i.e. 3PI/2).
    arc(x, y, 0.24, 0.36, 0.15 * PI, 1.35 * PI)
        // An arrowhead at the top end of the arc (pointing up).
        || tri(x, y, (0.5, 0.06), (0.36, 0.2), (0.58, 0.22))
}

/// A diagonal double-headed arrow (scale/resize).
fn icon_scale(x: f32, y: f32) -> bool {
    line(x, y, (0.28, 0.72), (0.72, 0.28), 0.1)
        || tri(x, y, (0.16, 0.84), (0.16, 0.58), (0.42, 0.84))
        || tri(x, y, (0.84, 0.16), (0.84, 0.42), (0.58, 0.16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_covers_some_but_not_all_pixels() {
        // A sanity check that each predicate draws something recognizable (a
        // non-trivial coverage fraction), so a broken predicate is caught.
        for icon in [
            icon_add as fn(f32, f32) -> bool,
            icon_save,
            icon_load,
            icon_select,
            icon_move,
            icon_rotate,
            icon_scale,
        ] {
            let rgba = rasterize(icon);
            let covered = rgba.iter().skip(3).step_by(4).filter(|&&a| a > 0).count();
            let total = SIZE * SIZE;
            assert!(
                covered > total / 40 && covered < total * 9 / 10,
                "icon coverage {covered}/{total} is out of range"
            );
        }
    }
}
