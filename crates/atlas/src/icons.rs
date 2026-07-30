//! atlas icons: a tiny software rasterizer that draws the toolbar's icons into
//! white-on-transparent bitmaps once at startup, registered as Aurora images.
//!
//! Each icon is a boolean coverage predicate in a normalized `[0, 1]` square
//! (union of simple shapes - capsules, triangles, polygons, discs, rects, and
//! stroked circular arcs). The rasterizer supersamples it into an alpha mask;
//! the RGB is white, so the image pipeline's tint can recolor it later. No new
//! GPU pipeline, no bundled font or art assets - the "custom Aurora icon" path
//! from the ideatank.
//!
//! Curved marks (the eye, the rotate/refresh arrows, the magnet) are built from
//! [`arc_stroke`] - a wrap-safe stroked arc that can be centered anywhere - and
//! [`arc_arrow`], which caps an arc with a tangent-aligned head. That is what
//! keeps the round icons smooth rather than faceted.
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

/// The rendered pixel size of each icon (square). Sized for the largest place
/// an icon is shown (the file explorer's preview grid at 64px), so it is crisp
/// there; the small toolbar/tree uses (15-22px) downscale cleanly from this
/// anti-aliased source.
const SIZE: usize = 64;
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
    /// A disclosure triangle pointing right (a collapsed tree node).
    ChevronRight,
    /// A disclosure triangle pointing down (an expanded tree node).
    ChevronDown,
    /// An open eye (a visible node's visibility toggle).
    Eye,
    /// A struck-through eye (a hidden node's visibility toggle).
    EyeOff,
    /// A 3x3 grid (the viewport grid toggle).
    Grid,
    /// Two arrows from an origin (the world-axes toggle).
    Axes,
    /// A horseshoe magnet (the snapping toggle).
    Snap,
    /// A closed folder (a directory in the file explorer's tree).
    Folder,
    /// An open folder (the selected directory).
    FolderOpen,
    /// A page with a small picture (an image asset).
    FileImage,
    /// A page with a little node graph (a scene file).
    FileScene,
    /// A page with a code chevron pair (a Comet script).
    FileScript,
    /// A plain document with text lines (any other file).
    FileGeneric,
    /// A circular arrow (re-scan the project's files).
    Refresh,
    /// Stacked rows (the explorer's compact list view).
    ViewList,
    /// A 2x2 of squares (the explorer's grid/preview view).
    ViewGrid,
    /// An X (a modal's close button).
    Close,
    /// A gear/cog (opens the settings dialog).
    Settings,
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
    (Icon::ChevronRight, icon_chevron_right),
    (Icon::ChevronDown, icon_chevron_down),
    (Icon::Eye, icon_eye),
    (Icon::EyeOff, icon_eye_off),
    (Icon::Grid, icon_grid),
    (Icon::Axes, icon_axes),
    (Icon::Snap, icon_snap),
    (Icon::Folder, icon_folder),
    (Icon::FolderOpen, icon_folder_open),
    (Icon::FileImage, icon_file_image),
    (Icon::FileScene, icon_file_scene),
    (Icon::FileScript, icon_file_script),
    (Icon::FileGeneric, icon_file_generic),
    (Icon::Refresh, icon_refresh),
    (Icon::ViewList, icon_view_list),
    (Icon::ViewGrid, icon_view_grid),
    (Icon::Close, icon_close),
    (Icon::Settings, icon_settings),
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

/// A rectangle `[x0, x1] x [y0, y1]`.
fn rect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

/// A filled disc of radius `r` centered at `c`.
fn disc(x: f32, y: f32, c: (f32, f32), r: f32) -> bool {
    (x - c.0).hypot(y - c.1) <= r
}

/// A filled annulus about `c`: the ring between radii `rin` and `rout` (a disc
/// with a hole - e.g. a gear body or the eye's iris).
fn annulus(x: f32, y: f32, c: (f32, f32), rin: f32, rout: f32) -> bool {
    let d = (x - c.0).hypot(y - c.1);
    (rin..=rout).contains(&d)
}

/// A thin rectangular outline (four capsule edges of width `w`): the "page" the
/// file icons are drawn on, so their interior symbols read against empty space.
fn frame(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, w: f32) -> bool {
    line(x, y, (x0, y0), (x1, y0), w)
        || line(x, y, (x0, y1), (x1, y1), w)
        || line(x, y, (x0, y0), (x0, y1), w)
        || line(x, y, (x1, y0), (x1, y1), w)
}

/// The outline of a simple polygon (each edge a capsule of width `w`), used for
/// crisp line-art silhouettes like the floppy-disk body.
fn outline(x: f32, y: f32, pts: &[(f32, f32)], w: f32) -> bool {
    pts.iter()
        .zip(pts.iter().cycle().skip(1))
        .take(pts.len())
        .any(|(&a, &b)| line(x, y, a, b, w))
}

/// The point on the circle of radius `r` about `c` at angle `a` (radians, y-down
/// so `0` is +x, `PI/2` is down, `-PI/2` is up).
fn on_circle(c: (f32, f32), r: f32, a: f32) -> (f32, f32) {
    (c.0 + r * a.cos(), c.1 + r * a.sin())
}

/// A stroked circular arc: within `w/2` of the circle of radius `r` about `c`,
/// and within the clockwise angular sweep of `sweep` radians starting at `start`
/// (y-down, so increasing angle turns clockwise). Wrap-safe - the sweep may
/// cross the atan2 seam - and, unlike a center-fixed helper, it can sit anywhere,
/// which is what the eye lids and the round arrows need.
fn arc_stroke(x: f32, y: f32, c: (f32, f32), r: f32, start: f32, sweep: f32, w: f32) -> bool {
    let (dx, dy) = (x - c.0, y - c.1);
    if (dx.hypot(dy) - r).abs() > w * 0.5 {
        return false;
    }
    (dy.atan2(dx) - start).rem_euclid(std::f32::consts::TAU) <= sweep
}

/// An arrowhead capping a circular arc at angle `a` on the circle of radius `r`
/// about `c`. Its base is radial - spanning the ring's stroke at `a` - and its
/// tip runs `len` forward along the tangent (clockwise if `cw`), so the arc
/// flows straight into the point instead of a head stuck on sideways. The arc
/// should end at `a`; the head continues from there.
fn arc_arrow(x: f32, y: f32, c: (f32, f32), r: f32, a: f32, cw: bool, len: f32) -> bool {
    // The clockwise tangent (increasing angle) is (-sin, cos); reverse for ccw.
    let tangent = if cw {
        (-a.sin(), a.cos())
    } else {
        (a.sin(), -a.cos())
    };
    let half = len * 0.62; // base half-width, radial across the stroke
    let inner = on_circle(c, r - half, a);
    let outer = on_circle(c, r + half, a);
    let mid = on_circle(c, r, a);
    let tip = (mid.0 + tangent.0 * len, mid.1 + tangent.1 * len);
    tri(x, y, inner, outer, tip)
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

/// A floppy disk (save): a beveled-corner body outline, the top shutter, and a
/// label panel - the universal save mark, and clearly not a download arrow.
fn icon_save(x: f32, y: f32) -> bool {
    const BODY: [(f32, f32); 5] = [
        (0.18, 0.17),
        (0.65, 0.17),
        (0.83, 0.35),
        (0.83, 0.83),
        (0.18, 0.83),
    ];
    // Body outline.
    if outline(x, y, &BODY, 0.06) {
        return true;
    }
    // The metal shutter at the top (a solid block, offset toward the bevel).
    if rect(x, y, 0.5, 0.17, 0.68, 0.37) {
        return true;
    }
    // The write-protect label: an outlined panel filling the lower half.
    frame(x, y, 0.31, 0.52, 0.7, 0.78, 0.05)
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

/// Four arrows radiating from the center (move), with bold heads at the edges.
fn icon_move(x: f32, y: f32) -> bool {
    line(x, y, (0.5, 0.24), (0.5, 0.76), STROKE)
        || line(x, y, (0.24, 0.5), (0.76, 0.5), STROKE)
        || arrowhead(x, y, (0.5, 0.08), (0.0, -1.0), 0.21, 0.15)
        || arrowhead(x, y, (0.5, 0.92), (0.0, 1.0), 0.21, 0.15)
        || arrowhead(x, y, (0.08, 0.5), (-1.0, 0.0), 0.21, 0.15)
        || arrowhead(x, y, (0.92, 0.5), (1.0, 0.0), 0.21, 0.15)
}

/// A circular arrow (rotate): an ~80%-closed ring with a large head on the top
/// end, pointing along the ring's clockwise tangent.
fn icon_rotate(x: f32, y: f32) -> bool {
    use std::f32::consts::{PI, TAU};
    let c = (0.5, 0.5);
    let r = 0.3;
    let gap = 0.2 * TAU; // ~72-degree opening at the top
    let start = -PI / 2.0 + gap; // just clockwise of the top
    let sweep = TAU - gap; // ~80% of the circle
    arc_stroke(x, y, c, r, start, sweep, 0.08) || arc_arrow(x, y, c, r, start + sweep, true, 0.23)
}

/// A diagonal double-headed arrow (scale/resize), thin with bold heads.
fn icon_scale(x: f32, y: f32) -> bool {
    line(x, y, (0.28, 0.72), (0.72, 0.28), STROKE)
        || arrowhead(x, y, (0.15, 0.85), (-1.0, 1.0), 0.26, 0.17)
        || arrowhead(x, y, (0.85, 0.15), (1.0, -1.0), 0.26, 0.17)
}

/// A solid triangle pointing right (a collapsed tree node).
fn icon_chevron_right(x: f32, y: f32) -> bool {
    tri(x, y, (0.3, 0.16), (0.3, 0.84), (0.78, 0.5))
}

/// A solid triangle pointing down (an expanded tree node).
fn icon_chevron_down(x: f32, y: f32) -> bool {
    tri(x, y, (0.16, 0.3), (0.84, 0.3), (0.5, 0.78))
}

/// An open eye: a smooth almond (two circular-arc lids meeting at the corners)
/// with a filled pupil - a visible node's toggle.
fn icon_eye(x: f32, y: f32) -> bool {
    let w = 0.06;
    // The lids are arcs of two equal circles centered below and above the eye,
    // so each bows past the y=0.5 midline; clipping to a half keeps just the
    // near lid. They meet exactly at the corners (0.12, 0.5) and (0.88, 0.5).
    let top = ring_band(x, y, (0.5, 0.79), 0.478, w) && y <= 0.5;
    let bottom = ring_band(x, y, (0.5, 0.21), 0.478, w) && y >= 0.5;
    top || bottom || disc(x, y, (0.5, 0.5), 0.12)
}

/// A struck-through eye (a hidden node): the eye plus a diagonal slash.
fn icon_eye_off(x: f32, y: f32) -> bool {
    icon_eye(x, y) || line(x, y, (0.16, 0.18), (0.84, 0.82), 0.075)
}

/// A thin circular outline: within `w/2` of the circle of radius `r` about `c`.
fn ring_band(x: f32, y: f32, c: (f32, f32), r: f32, w: f32) -> bool {
    ((x - c.0).hypot(y - c.1) - r).abs() <= w * 0.5
}

/// A 3x3 grid of lines (the viewport grid toggle).
fn icon_grid(x: f32, y: f32) -> bool {
    let w = 0.08;
    line(x, y, (0.35, 0.1), (0.35, 0.9), w)
        || line(x, y, (0.65, 0.1), (0.65, 0.9), w)
        || line(x, y, (0.1, 0.35), (0.9, 0.35), w)
        || line(x, y, (0.1, 0.65), (0.9, 0.65), w)
}

/// Two arrows from a shared origin (up and right): the world X/Y axes.
fn icon_axes(x: f32, y: f32) -> bool {
    let o = (0.18, 0.82);
    line(x, y, o, (0.18, 0.18), STROKE)
        || line(x, y, o, (0.82, 0.82), STROKE)
        || arrowhead(x, y, (0.18, 0.07), (0.0, -1.0), 0.2, 0.14)
        || arrowhead(x, y, (0.93, 0.82), (1.0, 0.0), 0.2, 0.14)
}

/// A horseshoe magnet (the snapping toggle): a thick U with short arms, then a
/// small gap, then a square pole tip under each arm - the striped-pole look that
/// says "magnet" rather than a plain U.
fn icon_snap(x: f32, y: f32) -> bool {
    use std::f32::consts::PI;
    let c = (0.5, 0.4);
    let r = 0.26; // mid-radius of the band
    let t = 0.16; // band (and pole) width
    let arm_end = 0.58; // arms stop here (short)
    let pole_top = 0.64; // poles start below a small gap
    let pole_bot = pole_top + t; // square poles (side = arm width)
    let (lx, rx) = (c.0 - r, c.0 + r);
    // The curved back: the upper semicircle, from left (PI) clockwise over the
    // top to right (TAU).
    arc_stroke(x, y, c, r, PI, PI, t)
        // Two short arms down from the arc ends.
        || rect(x, y, lx - t * 0.5, c.1, lx + t * 0.5, arm_end)
        || rect(x, y, rx - t * 0.5, c.1, rx + t * 0.5, arm_end)
        // Square pole tips, arm-width, set off by the gap.
        || rect(x, y, lx - t * 0.5, pole_top, lx + t * 0.5, pole_bot)
        || rect(x, y, rx - t * 0.5, pole_top, rx + t * 0.5, pole_bot)
}

/// A closed folder: a back tab and the front body (a filled silhouette).
fn icon_folder(x: f32, y: f32) -> bool {
    rect(x, y, 0.12, 0.24, 0.46, 0.34) || rect(x, y, 0.12, 0.30, 0.88, 0.78)
}

/// An open folder (the selected directory): a tab and a thin back rim across the
/// top, then a symmetric front pocket (a trapezoid narrowing toward its base) -
/// the back-behind-front read that sets it apart from the flat closed folder,
/// kept symmetric so it does not look lopsided.
fn icon_folder_open(x: f32, y: f32) -> bool {
    // Tab + a back rim: the top edge of the folder, showing behind the pocket.
    rect(x, y, 0.14, 0.22, 0.44, 0.32)
        || rect(x, y, 0.12, 0.3, 0.88, 0.42)
        // The front pocket, symmetric: wider at the rim, tapering to the base.
        || poly(x, y, &[(0.1, 0.42), (0.9, 0.42), (0.78, 0.78), (0.22, 0.78)])
}

/// The document "page" the file icons share: a thin rounded-rectangle outline.
fn page(x: f32, y: f32) -> bool {
    frame(x, y, 0.24, 0.12, 0.76, 0.88, 0.05)
}

/// An image file: a page with a sun and a mountain (the universal picture mark).
fn icon_file_image(x: f32, y: f32) -> bool {
    page(x, y) || disc(x, y, (0.4, 0.34), 0.06) || tri(x, y, (0.3, 0.72), (0.5, 0.46), (0.7, 0.72))
}

/// A scene file: a page with a small node tree (one parent, two children) -
/// exactly what a scene is.
fn icon_file_scene(x: f32, y: f32) -> bool {
    page(x, y)
        || disc(x, y, (0.5, 0.32), 0.055)
        || disc(x, y, (0.36, 0.62), 0.055)
        || disc(x, y, (0.64, 0.62), 0.055)
        || line(x, y, (0.5, 0.32), (0.36, 0.62), 0.035)
        || line(x, y, (0.5, 0.32), (0.64, 0.62), 0.035)
}

/// A script file: a page marked with a code chevron pair, so a `.cmt` reads as
/// something you write rather than as any other document.
fn icon_file_script(x: f32, y: f32) -> bool {
    page(x, y)
        || line(x, y, (0.45, 0.39), (0.34, 0.53), 0.045)
        || line(x, y, (0.34, 0.53), (0.45, 0.67), 0.045)
        || line(x, y, (0.55, 0.39), (0.66, 0.53), 0.045)
        || line(x, y, (0.66, 0.53), (0.55, 0.67), 0.045)
}

/// A generic file: a page with three text lines.
fn icon_file_generic(x: f32, y: f32) -> bool {
    page(x, y)
        || line(x, y, (0.34, 0.4), (0.66, 0.4), 0.05)
        || line(x, y, (0.34, 0.54), (0.66, 0.54), 0.05)
        || line(x, y, (0.34, 0.68), (0.58, 0.68), 0.05)
}

/// A two-arrow circular refresh mark: two opposing arcs, each capped with a
/// tangent head, with the openings top and bottom - reads as "reload", not the
/// single-arrow rotate tool.
fn icon_refresh(x: f32, y: f32) -> bool {
    use std::f32::consts::{PI, TAU};
    let c = (0.5, 0.5);
    let r = 0.3;
    let w = 0.075;
    let gap = 0.14 * TAU; // matching openings at top and bottom
    let start = -PI / 2.0 + gap * 0.5; // just clockwise of the top
    let sweep = PI - gap; // each arc spans a little under a half-turn
    arc_stroke(x, y, c, r, start, sweep, w)
        || arc_stroke(x, y, c, r, start + PI, sweep, w)
        || arc_arrow(x, y, c, r, start + sweep, true, 0.2)
        || arc_arrow(x, y, c, r, start + PI + sweep, true, 0.2)
}

/// Three stacked rows, each a leading dot and a line (the compact list view).
fn icon_view_list(x: f32, y: f32) -> bool {
    let row = |cy: f32| disc(x, y, (0.23, cy), 0.055) || line(x, y, (0.36, cy), (0.84, cy), 0.08);
    row(0.3) || row(0.5) || row(0.7)
}

/// A 2x2 of squares (the grid / large-preview view).
fn icon_view_grid(x: f32, y: f32) -> bool {
    let cell = |x0: f32, y0: f32| rect(x, y, x0, y0, x0 + 0.28, y0 + 0.28);
    cell(0.16, 0.16) || cell(0.56, 0.16) || cell(0.16, 0.56) || cell(0.56, 0.56)
}

/// An X (a modal's close button), the same stroke width as the plus.
fn icon_close(x: f32, y: f32) -> bool {
    line(x, y, (0.26, 0.26), (0.74, 0.74), 0.13) || line(x, y, (0.74, 0.26), (0.26, 0.74), 0.13)
}

/// A gear / cog (opens settings): a solid ring body (a hub hole in the middle)
/// with eight blocky trapezoidal teeth around the rim - a cog, not a wheel of
/// thin spokes.
fn icon_settings(x: f32, y: f32) -> bool {
    use std::f32::consts::TAU;
    let c = (0.5, 0.5);
    let body = 0.29; // outer radius of the ring body
    let tip = 0.44; // radius the teeth reach
    let hub = 0.13; // the center hole
    // The body: a filled ring with a hub hole.
    if annulus(x, y, c, hub, body) {
        return true;
    }
    // Eight trapezoidal teeth, clearly wider where they join the body and
    // tapering to a narrower tip.
    const TEETH: usize = 8;
    let base_hw = 0.24; // base half-angle (wide, at the rim)
    let tip_hw = 0.085; // tip half-angle (narrow)
    for k in 0..TEETH {
        let a = (k as f32 + 0.5) / TEETH as f32 * TAU;
        let quad = [
            on_circle(c, body - 0.02, a - base_hw),
            on_circle(c, tip, a - tip_hw),
            on_circle(c, tip, a + tip_hw),
            on_circle(c, body - 0.02, a + base_hw),
        ];
        if poly(x, y, &quad) {
            return true;
        }
    }
    false
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

    /// Box-downsample a 64px icon mask to `n`x`n` (its alpha channel only), so
    /// the preview can show how an icon reads at toolbar size.
    fn downsample_alpha(mask: &[u8], n: usize) -> Vec<u8> {
        let mut small = vec![0u8; n * n];
        let step = SIZE / n;
        for sy in 0..n {
            for sx in 0..n {
                let mut sum = 0u32;
                for dy in 0..step {
                    for dx in 0..step {
                        let px = sx * step + dx;
                        let py = sy * step + dy;
                        sum += mask[(py * SIZE + px) * 4 + 3] as u32;
                    }
                }
                small[sy * n + sx] = (sum / (step * step) as u32) as u8;
            }
        }
        small
    }

    /// Render every icon into one PNG so a new or tweaked icon can be eyeballed:
    /// a grid (in [`SPECS`] order) of each icon at a large scale, and beside it
    /// the same icon downsampled to toolbar size, both white on a dark tile.
    /// Ignored by default; run with
    /// `cargo test -p atlas icons::tests::write_preview -- --ignored --nocapture`
    /// then open the printed path.
    #[test]
    #[ignore = "writes a preview PNG; run with --ignored to regenerate"]
    fn write_preview() {
        const COLS: usize = 6;
        const BIG: usize = 96; // large sample, per icon
        const SMALL: usize = 22; // toolbar-size sample
        const PAD: usize = 10;
        const CELL_W: usize = BIG + SMALL + PAD * 3;
        const CELL_H: usize = BIG + PAD * 2;
        let rows = SPECS.len().div_ceil(COLS);
        let (w, h) = (COLS * CELL_W, rows * CELL_H);
        let bg = image::Rgb([32, 34, 44]);
        let tile = image::Rgb([44, 47, 60]);
        let mut img = image::RgbImage::from_pixel(w as u32, h as u32, bg);

        for (idx, &(_, predicate)) in SPECS.iter().enumerate() {
            let mask = rasterize(predicate);
            let (col, row) = (idx % COLS, idx / COLS);
            let (cx, cy) = (col * CELL_W, row * CELL_H);
            let blit = |img: &mut image::RgbImage,
                        ox: usize,
                        oy: usize,
                        side: usize,
                        alpha: &dyn Fn(usize, usize) -> u32| {
                for yy in 0..side {
                    for xx in 0..side {
                        let a = alpha(xx, yy);
                        // Composite white (the icon) over the tile by coverage.
                        let c = |ch: usize| ((tile[ch] as u32 * (255 - a) + 255 * a) / 255) as u8;
                        img.put_pixel(
                            (ox + xx) as u32,
                            (oy + yy) as u32,
                            image::Rgb([c(0), c(1), c(2)]),
                        );
                    }
                }
            };
            // Large sample: nearest-upscale the 64px anti-aliased mask.
            let big_x = cx + PAD;
            let big_y = cy + PAD;
            blit(&mut img, big_x, big_y, BIG, &|xx, yy| {
                let sx = xx * SIZE / BIG;
                let sy = yy * SIZE / BIG;
                mask[(sy * SIZE + sx) * 4 + 3] as u32
            });
            // Small sample: a real box-downsample to toolbar size, top-aligned.
            let small = downsample_alpha(&mask, SMALL);
            let small_x = cx + PAD * 2 + BIG;
            let small_y = cy + PAD;
            blit(&mut img, small_x, small_y, SMALL, &|xx, yy| {
                small[yy * SMALL + xx] as u32
            });
        }

        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/icons_preview.png");
        img.save(path).expect("write preview");
        println!("icon preview written to {path}");
    }
}
