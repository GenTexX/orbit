//! atlas viewport interaction: the editor pan/zoom camera, the selection gizmo
//! (move arrows, rotate, uniform and per-axis scale), and the pure math each
//! drag applies (M3 step 7). Everything here is headless and unit-tested;
//! main.rs only routes input into it.
//!
//! A sprite is centered on its node's origin (ADR 0019), so every drag edits
//! exactly one transform property: rotating rotates, scaling scales, and the
//! translation - the sprite's center - never moves under either.

use glam::Vec2;
use helios::{Component, NodeId, Scene, Transform};
use photon::{Camera, Color, Sprite};

/// The viewport's themeable colors (in photon color space), built from the
/// editor theme each frame and passed to the drawing functions. Only colors live
/// here; sizes/alphas below stay module constants.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// The selected sprite's outline (and the rotate stem).
    pub outline: Color,
    /// The engine axis palette: X red, Y green (move arrows, scale handles, and
    /// the origin grid axes).
    pub axis_x: Color,
    pub axis_y: Color,
    /// The rotate handle (blue) and the uniform-scale corner handle (amber).
    pub rotate_handle: Color,
    pub scale_handle: Color,
    /// Minor and major grid lines.
    pub grid_minor: Color,
    pub grid_major: Color,
}

impl Palette {
    /// The built-in defaults, used by the headless tests (the app builds the
    /// palette from the live theme instead).
    #[allow(dead_code)]
    pub const DEFAULT: Palette = Palette {
        outline: Color::new(0.30, 0.55, 0.90, 1.0),
        axis_x: Color::new(0.90, 0.32, 0.32, 1.0),
        axis_y: Color::new(0.35, 0.70, 0.38, 1.0),
        rotate_handle: Color::new(0.35, 0.55, 1.0, 1.0),
        scale_handle: Color::new(1.0, 0.75, 0.25, 1.0),
        grid_minor: Color::new(0.0, 0.0, 0.0, 0.10),
        grid_major: Color::new(0.0, 0.0, 0.0, 0.22),
    };
}

/// Gizmo metrics in *screen* pixels (divided by zoom at use, so the gizmo
/// keeps its size as the view zooms). Grab radii are deliberately larger than
/// the visuals - the same lesson as the too-thin splitter bars.
const OUTLINE_PX: f32 = 2.0;
const HANDLE_HALF_PX: f32 = 5.0;
const GRAB_RADIUS_PX: f32 = 10.0;
/// Length of the X/Y move arrows, from the center.
const ARROW_PX: f32 = 56.0;
/// How far above the sprite's top edge the rotate handle floats.
const ROTATE_OFFSET_PX: f32 = 26.0;
/// Thickness of the full-viewport guide line shown during an axis drag.
const GUIDE_PX: f32 = 1.5;
/// The guide line's translucency (drawn in the dragged axis's color).
const GUIDE_ALPHA: f32 = 0.55;

/// Editor grid metrics. Lines are 1 screen-px; the origin axes a touch thicker.
const GRID_LINE_PX: f32 = 1.0;
const GRID_AXIS_PX: f32 = 1.5;
/// Adaptive spacing target: the on-screen gap between major lines is kept at or
/// above this, so the grid never gets denser than this as the view zooms out.
const GRID_MIN_PX: f32 = 48.0;
/// Minor (1/5) subdivisions are only drawn while their on-screen gap is at least
/// this - below it they would read as noise.
const GRID_MINOR_MIN_PX: f32 = 9.0;

/// The editor's pan/zoom view of the scene: `pan` is the world coordinate at
/// the viewport's top-left (photon's own camera anchor), `zoom` is screen
/// pixels per world unit (1.0 = 1:1).
#[derive(Debug, Clone, Copy)]
pub struct EditorCamera {
    pub pan: Vec2,
    pub zoom: f32,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl EditorCamera {
    /// The photon camera rendering this view into a `viewport_px`-sized target:
    /// zooming in means showing a smaller world region on the same pixels.
    pub fn camera(&self, viewport_px: Vec2) -> Camera {
        Camera::new(self.pan, viewport_px / self.zoom)
    }

    /// A viewport-local pixel position to world coordinates.
    pub fn screen_to_world(&self, screen: Vec2) -> Vec2 {
        self.pan + screen / self.zoom
    }

    /// World coordinates to a viewport-local pixel position.
    #[allow(
        dead_code,
        reason = "screen_to_world's inverse; used by tests now, overlays later"
    )]
    pub fn world_to_screen(&self, world: Vec2) -> Vec2 {
        (world - self.pan) * self.zoom
    }

    /// Pan by a screen-pixel delta (dragging the view; content follows the
    /// cursor, so the camera moves opposite the drag).
    pub fn pan_by_screen(&mut self, delta: Vec2) {
        self.pan -= delta / self.zoom;
    }

    /// Zoom by `factor` about a viewport-local pixel, keeping the world point
    /// under the cursor fixed on screen (zoom toward what you point at).
    pub fn zoom_about(&mut self, screen: Vec2, factor: f32) {
        let anchor = self.screen_to_world(screen);
        self.zoom = (self.zoom * factor).clamp(0.1, 10.0);
        self.pan = anchor - screen / self.zoom;
    }
}

/// The selected node's gizmo, in world space: the sprite's oriented frame and
/// every handle's anchor point.
pub struct Gizmo {
    /// The node's origin = the sprite's center (ADR 0019) - the pivot every
    /// rotate and scale works about, for free.
    pub center: Vec2,
    /// World top-left corner of the quad (for the outline).
    anchor: Vec2,
    /// The world-space quad size and rotation.
    size: Vec2,
    angle: f32,
    /// World-space unit axes of the sprite's local X and Y.
    pub axis_x: Vec2,
    pub axis_y: Vec2,
    /// Handle anchor points, world space.
    pub rotate_center: Vec2,
    pub scale_corner: Vec2,
    pub scale_x: Vec2,
    pub scale_y: Vec2,
    pub arrow_x_end: Vec2,
    pub arrow_y_end: Vec2,
    /// Handle grab radius in world units.
    pub grab_radius: f32,
}

/// The active transform tool, chosen from the viewport toolbar. It filters
/// which gizmo handles are shown and hittable; the sprite body always drags to
/// move, whatever the mode. `Select` shows the full gizmo (every handle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum GizmoMode {
    #[default]
    Select,
    Move,
    Rotate,
    Scale,
}

impl GizmoMode {
    /// Every mode, in toolbar order.
    pub const ALL: [GizmoMode; 4] = [
        GizmoMode::Select,
        GizmoMode::Move,
        GizmoMode::Rotate,
        GizmoMode::Scale,
    ];

    /// The toolbar caption.
    pub fn label(self) -> &'static str {
        match self {
            GizmoMode::Select => "Select",
            GizmoMode::Move => "Move",
            GizmoMode::Rotate => "Rotate",
            GizmoMode::Scale => "Scale",
        }
    }

    /// Whether this mode shows and activates the given handle.
    fn shows(self, hit: GizmoHit) -> bool {
        match self {
            GizmoMode::Select => true,
            GizmoMode::Move => matches!(hit, GizmoHit::MoveX | GizmoHit::MoveY),
            GizmoMode::Rotate => matches!(hit, GizmoHit::Rotate),
            GizmoMode::Scale => matches!(
                hit,
                GizmoHit::ScaleCorner | GizmoHit::ScaleX | GizmoHit::ScaleY
            ),
        }
    }
}

/// Which gizmo handle a press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoHit {
    /// The X move arrow: translate along the sprite's local X only.
    MoveX,
    /// The Y move arrow: translate along the sprite's local Y only.
    MoveY,
    /// The rotate handle: rotate about the center.
    Rotate,
    /// The corner handle: free (per-axis) scale about the center, or uniform
    /// with Shift held - the app chooses the drag kind from the modifier.
    ScaleCorner,
    /// The right-edge handle: scale X only, about the center.
    ScaleX,
    /// The bottom-edge handle: scale Y only, about the center.
    ScaleY,
}

/// Build the gizmo for `node`, or `None` if it has no Sprite component to
/// frame (e.g. the root selected in the tree).
pub fn gizmo(scene: &Scene, node: NodeId, zoom: f32) -> Option<Gizmo> {
    // find_map, not map().next(): a node can carry a script alongside its
    // sprite, and the gizmo belongs to the sprite wherever in the list it sits.
    let sprite_size = scene.node(node).components.iter().find_map(|c| match c {
        Component::Sprite(s) => Some(s.size),
        // Neither a script nor a camera has an extent to frame. A camera is
        // still selectable and still movable through the inspector; it just has
        // no rectangle for a gizmo to grab.
        Component::Script(_) | Component::Camera(_) => None,
    })?;
    let affine = scene.world_transform(node);
    let (scale, angle, translation) = affine.to_scale_angle_translation();
    let size = scale * sprite_size;
    let half = sprite_size * 0.5;
    let axis_x = Vec2::from_angle(angle);
    let axis_y = axis_x.perp();
    // The quad is centered on the node's origin (ADR 0019): local space spans
    // -half .. +half, and the origin is the natural pivot.
    let center = translation;
    let arrow = ARROW_PX / zoom;

    Some(Gizmo {
        center,
        anchor: affine.transform_point2(-half),
        size,
        angle,
        axis_x,
        axis_y,
        rotate_center: center - axis_y * (size.y * 0.5 + ROTATE_OFFSET_PX / zoom),
        scale_corner: affine.transform_point2(half),
        scale_x: affine.transform_point2(Vec2::new(half.x, 0.0)),
        scale_y: affine.transform_point2(Vec2::new(0.0, half.y)),
        arrow_x_end: center + axis_x * arrow,
        arrow_y_end: center + axis_y * arrow,
        grab_radius: GRAB_RADIUS_PX / zoom,
    })
}

/// Distance from `p` to the segment `a..b`.
fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Which handle (if any) a world-space press lands on, restricted to those the
/// `mode` shows. Point handles win over the arrow shafts, and the shafts over
/// the sprite body (the caller falls back to picking).
pub fn hit_gizmo(gizmo: &Gizmo, world: Vec2, mode: GizmoMode) -> Option<GizmoHit> {
    let r = gizmo.grab_radius;
    let hits = [
        (GizmoHit::Rotate, gizmo.rotate_center),
        (GizmoHit::ScaleCorner, gizmo.scale_corner),
        (GizmoHit::ScaleX, gizmo.scale_x),
        (GizmoHit::ScaleY, gizmo.scale_y),
        (GizmoHit::MoveX, gizmo.arrow_x_end),
        (GizmoHit::MoveY, gizmo.arrow_y_end),
    ];
    for (hit, at) in hits {
        if mode.shows(hit) && world.distance(at) <= r {
            return Some(hit);
        }
    }
    // The arrow shafts, as segments (a slightly tighter radius than the
    // endpoint squares so they do not swallow clicks near the sprite body).
    if mode.shows(GizmoHit::MoveX)
        && segment_distance(world, gizmo.center, gizmo.arrow_x_end) <= r * 0.6
    {
        return Some(GizmoHit::MoveX);
    }
    if mode.shows(GizmoHit::MoveY)
        && segment_distance(world, gizmo.center, gizmo.arrow_y_end) <= r * 0.6
    {
        return Some(GizmoHit::MoveY);
    }
    None
}

/// The gizmo's overlay sprites (drawn with a solid-white texture, tinted): the
/// selection outline always, plus the handles the `mode` shows (move arrows,
/// rotate lollipop, scale handles), all sized in screen pixels so they stay
/// constant under zoom.
impl Gizmo {
    /// Whether `world` lies inside this node's quad, ignoring per-pixel alpha.
    ///
    /// A cheap CPU stand-in for the GPU pick: exact enough to say "this is the
    /// node you are hovering", without a render pass and a readback stall on
    /// every pointer move. The drop itself still uses the real pick.
    pub fn contains(&self, world: Vec2) -> bool {
        let offset = world - self.center;
        let half = self.size * 0.5;
        offset.dot(self.axis_x).abs() <= half.x && offset.dot(self.axis_y).abs() <= half.y
    }
}

/// The topmost node in `order` (back to front, as drawn) whose quad covers
/// `world`.
pub fn node_at(scene: &Scene, order: &[NodeId], world: Vec2, zoom: f32) -> Option<NodeId> {
    order
        .iter()
        .rev()
        .copied()
        .find(|&node| gizmo(scene, node, zoom).is_some_and(|g| g.contains(world)))
}

/// Just the four edges of a node's quad, in `color`: the cue for "this is the
/// node a drop would land on", without any of the gizmo's handles.
pub fn outline_sprites(gizmo: &Gizmo, zoom: f32, color: Color) -> Vec<Sprite> {
    let t = OUTLINE_PX / zoom;
    let (w, h) = (gizmo.size.x, gizmo.size.y);
    let rot = Vec2::from_angle(gizmo.angle);
    let at = |local: Vec2| gizmo.anchor + rot.rotate(local);
    let quad = |pos: Vec2, size: Vec2| {
        let mut s = Sprite::new(pos, size);
        s.rotation = gizmo.angle;
        s.tint = color;
        s
    };
    vec![
        quad(at(Vec2::ZERO), Vec2::new(w, t)),
        quad(at(Vec2::ZERO), Vec2::new(t, h)),
        quad(at(Vec2::new(0.0, h - t)), Vec2::new(w, t)),
        quad(at(Vec2::new(w - t, 0.0)), Vec2::new(t, h)),
    ]
}

pub fn gizmo_sprites(gizmo: &Gizmo, zoom: f32, mode: GizmoMode, pal: &Palette) -> Vec<Sprite> {
    let t = OUTLINE_PX / zoom;
    let half = HANDLE_HALF_PX / zoom;
    let (w, h) = (gizmo.size.x, gizmo.size.y);
    let rot = Vec2::from_angle(gizmo.angle);
    let at = |local: Vec2| gizmo.anchor + rot.rotate(local);

    let quad = |pos: Vec2, size: Vec2, tint: Color| {
        let mut s = Sprite::new(pos, size);
        s.rotation = gizmo.angle;
        s.tint = tint;
        s
    };
    // A small axis-aligned square centered on a point (handles read better
    // unrotated, and at this size the difference is invisible).
    let handle = |center: Vec2, tint: Color| {
        let mut s = Sprite::new(center - Vec2::splat(half), Vec2::splat(half * 2.0));
        s.tint = tint;
        s
    };

    let arrow = ARROW_PX / zoom;
    // The selection outline is always drawn - the four edges of the (possibly
    // rotated) selection rectangle.
    let mut sprites = vec![
        quad(at(Vec2::ZERO), Vec2::new(w, t), pal.outline),
        quad(at(Vec2::ZERO), Vec2::new(t, h), pal.outline),
        quad(at(Vec2::new(0.0, h - t)), Vec2::new(w, t), pal.outline),
        quad(at(Vec2::new(w - t, 0.0)), Vec2::new(t, h), pal.outline),
    ];
    if mode.shows(GizmoHit::MoveX) {
        // A shaft from the center along local X, with an endpoint square (red).
        sprites.push(quad(
            gizmo.center - gizmo.axis_y * (t * 0.5),
            Vec2::new(arrow, t),
            pal.axis_x,
        ));
        sprites.push(handle(gizmo.arrow_x_end, pal.axis_x));
    }
    if mode.shows(GizmoHit::MoveY) {
        sprites.push(quad(
            gizmo.center - gizmo.axis_x * (t * 0.5),
            Vec2::new(t, arrow),
            pal.axis_y,
        ));
        sprites.push(handle(gizmo.arrow_y_end, pal.axis_y));
    }
    if mode.shows(GizmoHit::Rotate) {
        // A stem from the top edge up to the rotate handle (blue).
        sprites.push(quad(
            at(Vec2::new(w * 0.5 - t * 0.5, -ROTATE_OFFSET_PX / zoom)),
            Vec2::new(t, ROTATE_OFFSET_PX / zoom),
            pal.outline,
        ));
        sprites.push(handle(gizmo.rotate_center, pal.rotate_handle));
    }
    // Gate each scale handle on its own hit, matching hit_gizmo (which tests
    // ScaleX/ScaleY/ScaleCorner independently) and the per-axis Move handles
    // above - so a mode that shows only some scale handles draws exactly those.
    if mode.shows(GizmoHit::ScaleX) {
        sprites.push(handle(gizmo.scale_x, pal.axis_x));
    }
    if mode.shows(GizmoHit::ScaleY) {
        sprites.push(handle(gizmo.scale_y, pal.axis_y));
    }
    if mode.shows(GizmoHit::ScaleCorner) {
        sprites.push(handle(gizmo.scale_corner, pal.scale_handle));
    }
    sprites
}

/// The full-viewport guide line for an axis-constrained drag, in the dragged
/// axis's color: a translucent line through the motion (or scale) axis,
/// spanning the whole visible view - the visual cue for "you are locked to
/// this axis". `None` for drags with no axis to show.
pub fn axis_guide_sprite(
    drag: &Drag,
    scene: &Scene,
    camera: &EditorCamera,
    viewport_px: Vec2,
    pal: &Palette,
) -> Option<Sprite> {
    let (point, dir, vertical) = match *drag {
        // The node's current world origin always lies on the motion line.
        Drag::MoveAxis {
            node,
            axis_world,
            vertical,
            ..
        } => (
            scene.world_transform(node).transform_point2(Vec2::ZERO),
            axis_world,
            vertical,
        ),
        // A per-axis scale stretches along the axis through its pivot.
        Drag::ScaleAxis {
            pivot,
            axis_world,
            vertical,
            ..
        } => (pivot, axis_world, vertical),
        _ => return None,
    };

    // Long enough to cross the whole view from any on-screen point: the
    // visible world rectangle's diagonal, extended both ways.
    let len = (viewport_px / camera.zoom).length();
    let t = GUIDE_PX / camera.zoom;
    let color = if vertical { pal.axis_y } else { pal.axis_x };

    let mut s = Sprite::new(
        point - dir * len - dir.perp() * (t * 0.5),
        Vec2::new(len * 2.0, t),
    );
    s.rotation = dir.to_angle();
    s.tint = Color::new(color.r, color.g, color.b, GUIDE_ALPHA);
    Some(s)
}

/// The smallest "nice" step (a 1/2/5 times a power of ten) that is at least
/// `min`, so the grid spacing lands on round world units at every zoom.
fn nice_step(min: f32) -> f32 {
    if min <= 0.0 {
        return 1.0;
    }
    let pow = 10f32.powf(min.log10().floor());
    for m in [1.0, 2.0, 5.0] {
        if m * pow >= min {
            return m * pow;
        }
    }
    10.0 * pow
}

/// Editor grid + origin axes as world-space overlay quads (drawn like the gizmo
/// on the tintable white texture), bounded to the visible world rectangle so the
/// line count stays proportional to the screen, not the world. Major lines fall
/// on a 1/2/5 spacing chosen so their on-screen gap stays >= `GRID_MIN_PX`;
/// minor (1/5) lines are added while they are not too dense. `show_axes` draws
/// the world X (red) and Y (green) axes when the origin is in view.
pub fn grid_sprites(
    camera: &EditorCamera,
    viewport_px: Vec2,
    show_grid: bool,
    show_axes: bool,
    pal: &Palette,
) -> Vec<Sprite> {
    // Guard against a non-positive zoom from a corrupt editor.ron: a zero or
    // negative zoom would make the step NaN/negative and the line loops below
    // run forever (or never terminate at all).
    let zoom = camera.zoom.max(1.0e-3);
    let min = camera.pan;
    let max = camera.pan + viewport_px / zoom;
    let (width, height) = (max.x - min.x, max.y - min.y);
    let mut out = Vec::new();

    let mut line = |along_x: bool, coord: f32, t: f32, color: Color| {
        let mut s = if along_x {
            // A vertical line at world x = coord, spanning the visible height.
            Sprite::new(Vec2::new(coord - t * 0.5, min.y), Vec2::new(t, height))
        } else {
            // A horizontal line at world y = coord, spanning the visible width.
            Sprite::new(Vec2::new(min.x, coord - t * 0.5), Vec2::new(width, t))
        };
        s.tint = color;
        out.push(s);
    };

    if show_grid {
        let step = nice_step(GRID_MIN_PX / zoom);
        let minor = step / 5.0;
        let show_minor = minor * zoom >= GRID_MINOR_MIN_PX;
        let t = GRID_LINE_PX / zoom;
        // Minor lines first (skipping those that coincide with a major line), so
        // the brighter majors draw on top of them.
        if show_minor {
            for i in (min.x / minor).floor() as i64..=(max.x / minor).ceil() as i64 {
                if i % 5 != 0 {
                    line(true, i as f32 * minor, t, pal.grid_minor);
                }
            }
            for j in (min.y / minor).floor() as i64..=(max.y / minor).ceil() as i64 {
                if j % 5 != 0 {
                    line(false, j as f32 * minor, t, pal.grid_minor);
                }
            }
        }
        for i in (min.x / step).floor() as i64..=(max.x / step).ceil() as i64 {
            line(true, i as f32 * step, t, pal.grid_major);
        }
        for j in (min.y / step).floor() as i64..=(max.y / step).ceil() as i64 {
            line(false, j as f32 * step, t, pal.grid_major);
        }
    }

    if show_axes {
        let t = GRID_AXIS_PX / zoom;
        if (min.y..=max.y).contains(&0.0) {
            line(false, 0.0, t, pal.axis_x); // world X axis (horizontal) is red
        }
        if (min.x..=max.x).contains(&0.0) {
            line(true, 0.0, t, pal.axis_y); // world Y axis (vertical) is green
        }
    }
    out
}

/// An in-progress viewport drag. Each records the transform at press time and
/// applies its math from that original - a release commits one clean undo step.
/// With sprites centered on the node origin (ADR 0019), every drag edits
/// exactly one property; nothing compensates anything.
#[derive(Debug, Clone, Copy)]
pub enum Drag {
    /// Moving the node body freely: `grab_world` is where the press landed.
    Move {
        node: NodeId,
        original: Transform,
        grab_world: Vec2,
    },
    /// Moving along one world-space axis direction only (a gizmo arrow).
    /// `vertical` says which local axis (for the guide line's color).
    MoveAxis {
        node: NodeId,
        original: Transform,
        grab_world: Vec2,
        axis_world: Vec2,
        vertical: bool,
    },
    /// Rotating about the origin: `grab_angle` is the cursor's angle about
    /// `pivot` (the node's world origin at press time).
    Rotate {
        node: NodeId,
        original: Transform,
        pivot: Vec2,
        grab_angle: f32,
    },
    /// Uniformly scaling about the origin: `grab_dist` is the cursor's
    /// distance from the pivot at press time.
    ScaleUniform {
        node: NodeId,
        original: Transform,
        pivot: Vec2,
        grab_dist: f32,
    },
    /// Scaling both axes independently (the corner handle without Shift): each
    /// axis scales by the ratio of the cursor's projection onto it against the
    /// press-time projection.
    ScaleFree {
        node: NodeId,
        original: Transform,
        pivot: Vec2,
        axis_x: Vec2,
        axis_y: Vec2,
        grab_proj_x: f32,
        grab_proj_y: f32,
    },
    /// Scaling one local axis: the cursor's offset from the pivot is projected
    /// onto `axis_world`, and the ratio to the press-time projection scales
    /// that component.
    ScaleAxis {
        node: NodeId,
        original: Transform,
        pivot: Vec2,
        axis_world: Vec2,
        grab_proj: f32,
        vertical: bool,
    },
}

impl Drag {
    /// The node being dragged and its transform at press time.
    pub fn target(&self) -> (NodeId, Transform) {
        match *self {
            Drag::Move { node, original, .. }
            | Drag::MoveAxis { node, original, .. }
            | Drag::Rotate { node, original, .. }
            | Drag::ScaleUniform { node, original, .. }
            | Drag::ScaleFree { node, original, .. }
            | Drag::ScaleAxis { node, original, .. } => (node, original),
        }
    }

    /// The transform this drag yields with the cursor at `world` - pure math
    /// over the press-time original, so it never accumulates error across
    /// frames.
    pub fn apply(&self, scene: &Scene, world: Vec2) -> Transform {
        match *self {
            Drag::Move {
                node,
                original,
                grab_world,
            } => move_by(scene, node, original, world - grab_world),
            Drag::MoveAxis {
                node,
                original,
                grab_world,
                axis_world,
                ..
            } => {
                // Only the component of the cursor delta along the axis moves
                // the node - the constrained translate of a gizmo arrow.
                let along = (world - grab_world).dot(axis_world);
                move_by(scene, node, original, axis_world * along)
            }
            Drag::Rotate {
                original,
                pivot,
                grab_angle,
                ..
            } => {
                // The change in the cursor's angle about the pivot is the same
                // in world and parent-local space (a parent rotation offsets
                // both by a constant), so it adds directly. The pivot IS the
                // translation (ADR 0019): nothing else moves.
                Transform {
                    rotation: original.rotation + ((world - pivot).to_angle() - grab_angle),
                    ..original
                }
            }
            Drag::ScaleUniform {
                original,
                pivot,
                grab_dist,
                ..
            } => {
                let k = (world.distance(pivot) / grab_dist.max(1.0e-3)).max(0.05);
                Transform {
                    scale: original.scale * k,
                    ..original
                }
            }
            Drag::ScaleFree {
                original,
                pivot,
                axis_x,
                axis_y,
                grab_proj_x,
                grab_proj_y,
                ..
            } => {
                // Guard the divisor like ScaleUniform: grab_proj_* is the grab's
                // positive offset along each handle axis, ~0 only for a degenerate
                // zero-extent sprite, where an unguarded divide gives +inf scale
                // (.max(0.05) catches a NaN but not a positive infinity).
                let kx = ((world - pivot).dot(axis_x) / grab_proj_x.max(1.0e-3)).max(0.05);
                let ky = ((world - pivot).dot(axis_y) / grab_proj_y.max(1.0e-3)).max(0.05);
                Transform {
                    scale: original.scale * Vec2::new(kx, ky),
                    ..original
                }
            }
            Drag::ScaleAxis {
                original,
                pivot,
                axis_world,
                grab_proj,
                vertical,
                ..
            } => {
                let proj = (world - pivot).dot(axis_world);
                // Guard the divisor as in ScaleFree/ScaleUniform (see above).
                let k = (proj / grab_proj.max(1.0e-3)).max(0.05);
                let mut scale = original.scale;
                if vertical {
                    scale.y *= k;
                } else {
                    scale.x *= k;
                }
                Transform { scale, ..original }
            }
        }
    }
}

/// Round `v` to the nearest multiple of `step` (a no-op for a non-positive
/// step, so snapping can be turned off by zeroing an increment).
fn snap(v: f32, step: f32) -> f32 {
    if step <= 0.0 {
        v
    } else {
        (v / step).round() * step
    }
}

/// Snap the property a drag edits to its increment: translation to `move_step`,
/// rotation to `rotate_step_rad` radians, scale to `scale_step`. Translation is
/// snapped in the node's LOCAL space (its `Transform.translation`), so the grid
/// is world-aligned only for a node whose parent is untransformed - the common
/// case; under a rotated or scaled parent the increment follows the parent's
/// frame. Only the component the drag actually changes is snapped (a rotate
/// leaves the position alone, a per-axis scale snaps just that axis), so
/// snapping never disturbs a value the drag was not touching.
pub fn snap_transform(
    drag: &Drag,
    t: Transform,
    move_step: f32,
    rotate_step_rad: f32,
    scale_step: f32,
) -> Transform {
    match drag {
        // A free move lands the node's final position on the grid.
        Drag::Move { .. } => Transform {
            translation: Vec2::new(
                snap(t.translation.x, move_step),
                snap(t.translation.y, move_step),
            ),
            ..t
        },
        // An axis-constrained move snaps only the distance travelled ALONG the
        // constraint axis, so the perpendicular position it was locked out of is
        // never disturbed (the node stays exactly on the gizmo arrow's line).
        Drag::MoveAxis {
            original,
            axis_world,
            ..
        } => {
            let along = (t.translation - original.translation).dot(*axis_world);
            Transform {
                translation: original.translation + *axis_world * snap(along, move_step),
                ..t
            }
        }
        Drag::Rotate { .. } => Transform {
            rotation: snap(t.rotation, rotate_step_rad),
            ..t
        },
        // Uniform scale snaps one axis and matches the other by the same factor,
        // so it stays uniform.
        Drag::ScaleUniform { .. } => {
            let sx = snap_scale(t.scale.x, scale_step);
            let k = if t.scale.x.abs() > 1.0e-6 {
                sx / t.scale.x
            } else {
                1.0
            };
            Transform {
                scale: Vec2::new(sx, t.scale.y * k),
                ..t
            }
        }
        Drag::ScaleAxis { vertical, .. } => Transform {
            scale: if *vertical {
                Vec2::new(t.scale.x, snap_scale(t.scale.y, scale_step))
            } else {
                Vec2::new(snap_scale(t.scale.x, scale_step), t.scale.y)
            },
            ..t
        },
        Drag::ScaleFree { .. } => Transform {
            scale: Vec2::new(
                snap_scale(t.scale.x, scale_step),
                snap_scale(t.scale.y, scale_step),
            ),
            ..t
        },
    }
}

/// Snap a scale component to `step`, but never to zero (or below) - a snapped
/// scale of 0 would collapse the sprite. Floored at the same 0.05 the drags use.
fn snap_scale(v: f32, step: f32) -> f32 {
    snap(v, step).max(0.05)
}

/// Translate `original` by a world-space delta: the delta maps through the
/// inverse of the parent's world transform into the node's local space
/// (identity for a root child), so the node follows the cursor exactly even
/// under a rotated or scaled parent.
fn move_by(scene: &Scene, node: NodeId, original: Transform, world_delta: Vec2) -> Transform {
    let local_delta = match scene.parent(node) {
        Some(parent) => scene
            .world_transform(parent)
            .inverse()
            .transform_vector2(world_delta),
        None => world_delta,
    };
    Transform {
        translation: original.translation + local_delta,
        ..original
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios::{Node, SpriteComponent};

    const EPS: f32 = 1.0e-3;

    #[test]
    fn a_quad_contains_the_points_it_covers_and_rotates_with_the_node() {
        let (mut scene, node) = scene_with_sprite(Vec2::ZERO, Vec2::splat(100.0));
        let g = gizmo(&scene, node, 1.0).expect("a sprite has a quad");
        assert!(g.contains(Vec2::ZERO), "the center");
        assert!(g.contains(Vec2::new(49.0, 49.0)), "just inside a corner");
        assert!(!g.contains(Vec2::new(51.0, 0.0)), "just past the edge");

        // Rotated 45 degrees, a point past the corner falls outside while the
        // same distance along the new axis is still in - which is the whole
        // reason the test projects onto the axes instead of comparing bounds.
        scene.node_mut(node).transform.rotation = std::f32::consts::FRAC_PI_4;
        let g = gizmo(&scene, node, 1.0).unwrap();
        assert!(!g.contains(Vec2::new(48.0, 48.0)), "the corner swung away");
        assert!(g.contains(Vec2::new(0.0, 68.0)), "and the edge swung out");
    }

    #[test]
    fn node_at_finds_the_topmost_cover_and_nothing_over_empty_space() {
        let mut scene = Scene::new("root");
        let root = scene.root();
        let mut add = |x: f32| {
            let mut node = Node::new("sprite");
            node.transform = Transform::from_translation(Vec2::new(x, 0.0));
            node.components.push(Component::Sprite(SpriteComponent {
                texture: String::new(),
                tint: [1.0; 4],
                size: Vec2::splat(100.0),
            }));
            scene.add_child(root, node)
        };
        let under = add(0.0);
        let over = add(20.0); // overlaps `under`, drawn after it

        // In the overlap the later-drawn node wins - what you see on top is what
        // a drop lands on.
        assert_eq!(
            node_at(&scene, &[under, over], Vec2::new(10.0, 0.0), 1.0),
            Some(over)
        );
        assert_eq!(
            node_at(&scene, &[under, over], Vec2::new(-40.0, 0.0), 1.0),
            Some(under),
            "where only the lower one reaches"
        );
        assert_eq!(
            node_at(&scene, &[under, over], Vec2::new(500.0, 0.0), 1.0),
            None
        );
    }

    #[test]
    fn snapping_rounds_only_the_property_the_drag_edits() {
        let (scene, node) = scene_with_sprite(Vec2::ZERO, Vec2::splat(100.0));
        let orig = scene.node(node).transform;
        let step_rad = 15f32.to_radians();

        // Move snaps the final position to the grid, leaves rotation/scale alone.
        let mv = Drag::Move {
            node,
            original: orig,
            grab_world: Vec2::ZERO,
        };
        let t = Transform {
            translation: Vec2::new(35.0, -3.0),
            ..orig
        };
        let s = snap_transform(&mv, t, 16.0, step_rad, 0.1);
        assert_eq!(s.translation, Vec2::new(32.0, 0.0));
        assert_eq!(s.rotation, orig.rotation);
        assert_eq!(s.scale, orig.scale);
        // Snapping off (a zero step) is the identity.
        assert_eq!(
            snap_transform(&mv, t, 0.0, 0.0, 0.0).translation,
            t.translation
        );

        // Rotate snaps the angle (17deg -> 15deg), leaves position alone.
        let rot = Drag::Rotate {
            node,
            original: orig,
            pivot: Vec2::ZERO,
            grab_angle: 0.0,
        };
        let t = Transform {
            rotation: 17f32.to_radians(),
            ..orig
        };
        let s = snap_transform(&rot, t, 16.0, step_rad, 0.1);
        assert!((s.rotation - step_rad).abs() < EPS);
        assert_eq!(s.translation, orig.translation);

        // A per-axis scale snaps only its axis (horizontal -> x).
        let sx = Drag::ScaleAxis {
            node,
            original: orig,
            pivot: Vec2::ZERO,
            axis_world: Vec2::X,
            grab_proj: 1.0,
            vertical: false,
        };
        let t = Transform {
            scale: Vec2::new(1.37, 2.04),
            ..orig
        };
        let s = snap_transform(&sx, t, 16.0, step_rad, 0.1);
        assert!((s.scale.x - 1.4).abs() < EPS);
        assert_eq!(s.scale.y, 2.04);

        // Uniform scale snaps one axis and keeps the aspect ratio.
        let su = Drag::ScaleUniform {
            node,
            original: orig,
            pivot: Vec2::ZERO,
            grab_dist: 1.0,
        };
        let t = Transform {
            scale: Vec2::new(1.37, 1.37),
            ..orig
        };
        let s = snap_transform(&su, t, 16.0, step_rad, 0.1);
        assert!((s.scale.x - 1.4).abs() < EPS);
        assert!((s.scale.x - s.scale.y).abs() < EPS, "stays uniform");
    }

    #[test]
    fn axis_move_snaps_along_the_axis_and_never_touches_the_perpendicular() {
        let (scene, node) = scene_with_sprite(Vec2::ZERO, Vec2::splat(100.0));
        // Start with an off-grid perpendicular coordinate (y = 7) that snapping
        // to a 16-unit grid would round to 0 if it wrongly touched it.
        let orig = Transform {
            translation: Vec2::new(0.0, 7.0),
            ..scene.node(node).transform
        };
        let step_rad = 15f32.to_radians();

        // Horizontal arrow drag (vertical = false, axis = +X): the node has slid
        // to x = 35, y untouched at 7. Snapping rounds x to 32 and leaves y = 7.
        let dx = Drag::MoveAxis {
            node,
            original: orig,
            grab_world: orig.translation,
            axis_world: Vec2::X,
            vertical: false,
        };
        let t = Transform {
            translation: Vec2::new(35.0, 7.0),
            ..orig
        };
        let s = snap_transform(&dx, t, 16.0, step_rad, 0.1);
        assert_eq!(s.translation, Vec2::new(32.0, 7.0), "y perpendicular kept");

        // A rotated (diagonal) axis: the snapped result must stay exactly on the
        // axis line (zero perpendicular drift) with the along-distance snapped.
        let axis = Vec2::new(1.0, 1.0).normalize();
        let diag = Drag::MoveAxis {
            node,
            original: orig,
            grab_world: orig.translation,
            axis_world: axis,
            vertical: false,
        };
        let t = Transform {
            translation: orig.translation + axis * 20.0, // 20 along -> snaps to 16
            ..orig
        };
        let s = snap_transform(&diag, t, 16.0, step_rad, 0.1);
        let moved = s.translation - orig.translation;
        let perp = Vec2::new(-axis.y, axis.x);
        assert!(moved.dot(perp).abs() < EPS, "no perpendicular drift");
        assert!(
            (moved.length() - 16.0).abs() < EPS,
            "distance snapped to 16"
        );
    }

    #[test]
    fn scale_snapping_never_collapses_to_zero() {
        let (scene, node) = scene_with_sprite(Vec2::ZERO, Vec2::splat(100.0));
        let orig = scene.node(node).transform;
        // A small scale (0.05) with a coarse step (0.5) would round to 0 without
        // the floor, collapsing the sprite; snap_scale keeps it at 0.05.
        let sx = Drag::ScaleAxis {
            node,
            original: orig,
            pivot: Vec2::ZERO,
            axis_world: Vec2::X,
            grab_proj: 1.0,
            vertical: false,
        };
        let t = Transform {
            scale: Vec2::new(0.05, 1.0),
            ..orig
        };
        let s = snap_transform(&sx, t, 16.0, 0.0, 0.5);
        assert!(s.scale.x >= 0.05, "scale never snaps to zero");
    }

    #[test]
    fn nice_step_snaps_up_the_one_two_five_ladder() {
        assert_eq!(nice_step(1.0), 1.0);
        assert_eq!(nice_step(1.5), 2.0);
        assert_eq!(nice_step(3.0), 5.0);
        assert_eq!(nice_step(6.0), 10.0);
        assert_eq!(nice_step(0.3), 0.5);
        assert_eq!(nice_step(120.0), 200.0);
    }

    #[test]
    fn the_grid_stays_bounded_and_comfortably_spaced_across_zoom() {
        let view = Vec2::new(1280.0, 720.0);
        for zoom in [0.1, 0.5, 1.0, 3.0, 10.0] {
            let cam = EditorCamera {
                pan: Vec2::new(-123.0, 45.0),
                zoom,
            };
            let sprites = grid_sprites(&cam, view, true, true, &Palette::DEFAULT);
            // A screenful of lines, never the whole world: bounded well under a
            // few hundred quads at any zoom (verticals+horizontals, minor+major).
            assert!(
                sprites.len() < 600,
                "zoom {zoom}: {} grid sprites is too many",
                sprites.len()
            );
            // The major spacing keeps its on-screen gap >= the target.
            let step = nice_step(GRID_MIN_PX / zoom);
            assert!(step * zoom >= GRID_MIN_PX - EPS, "zoom {zoom} too dense");
        }
    }

    #[test]
    fn axes_show_only_when_the_origin_is_in_view() {
        let view = Vec2::new(800.0, 600.0);
        // Origin visible (pan negative so 0,0 is inside [pan, pan+view]).
        let cam = EditorCamera {
            pan: Vec2::new(-100.0, -100.0),
            zoom: 1.0,
        };
        // Axes-only (no grid) gives exactly the two origin axes.
        assert_eq!(
            grid_sprites(&cam, view, false, true, &Palette::DEFAULT).len(),
            2
        );
        // Nothing when both are off.
        assert!(grid_sprites(&cam, view, false, false, &Palette::DEFAULT).is_empty());
        // Panned so the origin is off-screen (top-left far into +x,+y world):
        // neither axis line is emitted.
        let off = EditorCamera {
            pan: Vec2::new(500.0, 500.0),
            zoom: 1.0,
        };
        assert!(grid_sprites(&off, view, false, true, &Palette::DEFAULT).is_empty());
    }

    fn scene_with_sprite(translation: Vec2, size: Vec2) -> (Scene, NodeId) {
        let mut scene = Scene::new("root");
        let root = scene.root();
        let mut node = Node::new("sprite");
        node.transform = Transform::from_translation(translation);
        node.components.push(Component::Sprite(SpriteComponent {
            texture: String::new(),
            tint: [1.0; 4],
            size,
        }));
        let id = scene.add_child(root, node);
        (scene, id)
    }

    /// The world center of `node`'s sprite quad - its origin (ADR 0019).
    fn world_center(scene: &Scene, node: NodeId) -> Vec2 {
        scene.world_transform(node).transform_point2(Vec2::ZERO)
    }

    #[test]
    fn screen_world_round_trips_under_pan_and_zoom() {
        let cam = EditorCamera {
            pan: Vec2::new(100.0, -40.0),
            zoom: 2.5,
        };
        let screen = Vec2::new(123.0, 45.0);
        let there_and_back = cam.world_to_screen(cam.screen_to_world(screen));
        assert!((there_and_back - screen).length() < EPS);
    }

    #[test]
    fn zoom_about_keeps_the_cursor_point_fixed() {
        let mut cam = EditorCamera {
            pan: Vec2::new(50.0, 50.0),
            ..Default::default()
        };
        let cursor = Vec2::new(200.0, 150.0);
        let world_before = cam.screen_to_world(cursor);
        cam.zoom_about(cursor, 1.5);
        let world_after = cam.screen_to_world(cursor);
        assert!((world_before - world_after).length() < EPS);
        assert!((cam.zoom - 1.5).abs() < EPS);
    }

    #[test]
    fn pan_moves_the_view_opposite_the_drag() {
        let mut cam = EditorCamera {
            zoom: 2.0,
            ..Default::default()
        };
        cam.pan_by_screen(Vec2::new(40.0, 0.0)); // drag content 40px right
        // The camera's world window moved left by 40 screen px = 20 world units.
        assert!((cam.pan - Vec2::new(-20.0, 0.0)).length() < EPS);
    }

    #[test]
    fn gizmo_frames_the_sprite_and_hits_its_handles() {
        let (scene, node) = scene_with_sprite(Vec2::new(100.0, 50.0), Vec2::new(60.0, 40.0));
        let g = gizmo(&scene, node, 1.0).expect("sprite node has a gizmo");

        // The node's origin (100, 50) is the quad's center (ADR 0019); the
        // quad spans (70, 30) .. (130, 70).
        assert!((g.center - Vec2::new(100.0, 50.0)).length() < EPS);
        assert!((g.scale_corner - Vec2::new(130.0, 70.0)).length() < EPS);
        assert!((g.scale_x - Vec2::new(130.0, 50.0)).length() < EPS);
        assert!((g.scale_y - Vec2::new(100.0, 70.0)).length() < EPS);
        // Unrotated: the rotate handle floats above the top mid-edge, and the
        // arrows point along +X and +Y from the center.
        assert!((g.rotate_center - Vec2::new(100.0, 30.0 - 26.0)).length() < EPS);
        assert!((g.arrow_x_end - Vec2::new(100.0 + 56.0, 50.0)).length() < EPS);
        assert!((g.arrow_y_end - Vec2::new(100.0, 50.0 + 56.0)).length() < EPS);

        assert_eq!(
            hit_gizmo(&g, g.rotate_center, GizmoMode::Select),
            Some(GizmoHit::Rotate)
        );
        assert_eq!(
            hit_gizmo(&g, g.scale_corner, GizmoMode::Select),
            Some(GizmoHit::ScaleCorner)
        );
        assert_eq!(
            hit_gizmo(&g, g.scale_x, GizmoMode::Select),
            Some(GizmoHit::ScaleX)
        );
        assert_eq!(
            hit_gizmo(&g, g.scale_y, GizmoMode::Select),
            Some(GizmoHit::ScaleY)
        );
        assert_eq!(
            hit_gizmo(&g, g.arrow_x_end, GizmoMode::Select),
            Some(GizmoHit::MoveX)
        );
        assert_eq!(
            hit_gizmo(&g, g.arrow_y_end, GizmoMode::Select),
            Some(GizmoHit::MoveY)
        );
        // A point along the X arrow shaft (between center and tip) hits too -
        // chosen clear of the right-edge scale handle at +30, which sits on
        // the same ray and rightly wins where they cross (point handles beat
        // shafts).
        assert_eq!(
            hit_gizmo(&g, g.center + g.axis_x * 45.0, GizmoMode::Select),
            Some(GizmoHit::MoveX)
        );
        assert_eq!(hit_gizmo(&g, Vec2::new(0.0, 0.0), GizmoMode::Select), None);
    }

    #[test]
    fn a_mode_hides_and_disables_other_handles() {
        let (scene, node) = scene_with_sprite(Vec2::new(100.0, 50.0), Vec2::new(60.0, 40.0));
        let g = gizmo(&scene, node, 1.0).unwrap();

        // Rotate mode: the rotate handle is live, the scale corner is not.
        assert_eq!(
            hit_gizmo(&g, g.rotate_center, GizmoMode::Rotate),
            Some(GizmoHit::Rotate)
        );
        assert_eq!(hit_gizmo(&g, g.scale_corner, GizmoMode::Rotate), None);
        assert_eq!(hit_gizmo(&g, g.arrow_x_end, GizmoMode::Rotate), None);

        // Move mode: arrows live, rotate/scale not.
        assert_eq!(
            hit_gizmo(&g, g.arrow_x_end, GizmoMode::Move),
            Some(GizmoHit::MoveX)
        );
        assert_eq!(hit_gizmo(&g, g.rotate_center, GizmoMode::Move), None);

        // The overlay only draws the outline plus the active mode's handles:
        // Rotate mode has fewer sprites than Select (which shows everything).
        let select = gizmo_sprites(&g, 1.0, GizmoMode::Select, &Palette::DEFAULT).len();
        let rotate = gizmo_sprites(&g, 1.0, GizmoMode::Rotate, &Palette::DEFAULT).len();
        assert!(rotate < select, "{rotate} should be fewer than {select}");
        // The outline (4 edges) is always present.
        assert!(gizmo_sprites(&g, 1.0, GizmoMode::Move, &Palette::DEFAULT).len() >= 4);
    }

    #[test]
    fn the_root_has_no_gizmo() {
        let (scene, _) = scene_with_sprite(Vec2::ZERO, Vec2::ONE);
        assert!(gizmo(&scene, scene.root(), 1.0).is_none());
    }

    #[test]
    fn a_move_drag_translates_by_the_world_delta() {
        let (scene, node) = scene_with_sprite(Vec2::new(10.0, 10.0), Vec2::new(20.0, 20.0));
        let drag = Drag::Move {
            node,
            original: scene.node(node).transform,
            grab_world: Vec2::new(15.0, 15.0),
        };
        let moved = drag.apply(&scene, Vec2::new(40.0, 5.0));
        assert!((moved.translation - Vec2::new(35.0, 0.0)).length() < EPS);
    }

    #[test]
    fn a_move_drag_respects_a_scaled_parent() {
        // The parent is scaled 2x, so a 30-world-unit drag is a 15-unit local
        // translation - the sprite still follows the cursor exactly.
        let (mut scene, node) = scene_with_sprite(Vec2::new(10.0, 10.0), Vec2::new(20.0, 20.0));
        let root = scene.root();
        scene.node_mut(root).transform = Transform {
            scale: Vec2::splat(2.0),
            ..Transform::IDENTITY
        };
        let drag = Drag::Move {
            node,
            original: scene.node(node).transform,
            grab_world: Vec2::new(20.0, 20.0),
        };
        let moved = drag.apply(&scene, Vec2::new(50.0, 20.0));
        assert!((moved.translation - Vec2::new(25.0, 10.0)).length() < EPS);
    }

    #[test]
    fn an_axis_move_only_translates_along_its_axis() {
        let (scene, node) = scene_with_sprite(Vec2::new(10.0, 10.0), Vec2::new(20.0, 20.0));
        let drag = Drag::MoveAxis {
            node,
            original: scene.node(node).transform,
            grab_world: Vec2::new(20.0, 20.0),
            axis_world: Vec2::X,
            vertical: false,
        };
        // A diagonal cursor move: only its X component applies.
        let moved = drag.apply(&scene, Vec2::new(50.0, 80.0));
        assert!((moved.translation - Vec2::new(40.0, 10.0)).length() < EPS);
    }

    #[test]
    fn rotation_pivots_about_the_origin_and_moves_nothing_else() {
        let (mut scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::new(40.0, 20.0));
        let center_before = world_center(&scene, node);
        let original = scene.node(node).transform;

        let drag = Drag::Rotate {
            node,
            original,
            pivot: center_before,
            grab_angle: 0.0,
        };
        // Drag to below the pivot: +90 degrees (Y-down world).
        let rotated = drag.apply(&scene, center_before + Vec2::new(0.0, 50.0));
        assert!((rotated.rotation - std::f32::consts::FRAC_PI_2).abs() < EPS);
        // The translation is untouched - rotating pivots about the origin,
        // which IS the sprite's center (ADR 0019).
        assert!((rotated.translation - original.translation).length() < EPS);

        scene.node_mut(node).transform = rotated;
        assert!((world_center(&scene, node) - center_before).length() < EPS);
    }

    #[test]
    fn uniform_scale_keeps_the_center_and_translation() {
        let (mut scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::new(40.0, 20.0));
        let center_before = world_center(&scene, node);
        let original = scene.node(node).transform;

        let drag = Drag::ScaleUniform {
            node,
            original,
            pivot: center_before,
            grab_dist: 20.0,
        };
        let scaled = drag.apply(&scene, center_before + Vec2::new(50.0, 0.0));
        assert!((scaled.scale - Vec2::splat(2.5)).length() < EPS);
        assert!((scaled.translation - original.translation).length() < EPS);

        scene.node_mut(node).transform = scaled;
        assert!((world_center(&scene, node) - center_before).length() < EPS);
    }

    #[test]
    fn axis_scale_changes_one_component_only() {
        let (scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::new(40.0, 20.0));
        let center = world_center(&scene, node);
        let original = scene.node(node).transform;

        // Grab the right-edge handle (20 world units from the center along X)
        // and drag it out to 40: X doubles, Y and translation untouched.
        let drag = Drag::ScaleAxis {
            node,
            original,
            pivot: center,
            axis_world: Vec2::X,
            grab_proj: 20.0,
            vertical: false,
        };
        let scaled = drag.apply(&scene, center + Vec2::new(40.0, 0.0));
        assert!((scaled.scale - Vec2::new(2.0, 1.0)).length() < EPS);
        assert!((scaled.translation - original.translation).length() < EPS);
    }

    #[test]
    fn a_free_corner_drag_scales_each_axis_independently() {
        let (scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::new(40.0, 20.0));
        let center = world_center(&scene, node);
        let original = scene.node(node).transform;

        // Grab the corner (at +20, +10 from the center); drag it to (+40, +5):
        // X doubles while Y halves - the axes move independently.
        let drag = Drag::ScaleFree {
            node,
            original,
            pivot: center,
            axis_x: Vec2::X,
            axis_y: Vec2::Y,
            grab_proj_x: 20.0,
            grab_proj_y: 10.0,
        };
        let scaled = drag.apply(&scene, center + Vec2::new(40.0, 5.0));
        assert!((scaled.scale - Vec2::new(2.0, 0.5)).length() < EPS);
        assert!((scaled.translation - original.translation).length() < EPS);
    }

    #[test]
    fn a_degenerate_zero_extent_scale_drag_stays_finite() {
        // A zero-extent sprite makes grab_proj ~0; the drag must not divide by it
        // into a NaN/infinite scale that would poison the transform.
        let (scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::ZERO);
        let center = world_center(&scene, node);
        let original = scene.node(node).transform;

        let axis = Drag::ScaleAxis {
            node,
            original,
            pivot: center,
            axis_world: Vec2::X,
            grab_proj: 0.0,
            vertical: false,
        };
        let s = axis.apply(&scene, center + Vec2::new(40.0, 0.0));
        assert!(s.scale.is_finite(), "axis scale must stay finite");

        let free = Drag::ScaleFree {
            node,
            original,
            pivot: center,
            axis_x: Vec2::X,
            axis_y: Vec2::Y,
            grab_proj_x: 0.0,
            grab_proj_y: 0.0,
        };
        let s = free.apply(&scene, center + Vec2::new(40.0, 5.0));
        assert!(s.scale.is_finite(), "free scale must stay finite");
    }

    #[test]
    fn an_axis_drag_shows_a_full_viewport_guide_line() {
        let (scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::new(40.0, 20.0));
        let cam = EditorCamera::default();
        let viewport_px = Vec2::new(800.0, 600.0);

        let drag = Drag::MoveAxis {
            node,
            original: scene.node(node).transform,
            grab_world: Vec2::new(100.0, 100.0),
            axis_world: Vec2::X,
            vertical: false,
        };
        let guide = axis_guide_sprite(&drag, &scene, &cam, viewport_px, &Palette::DEFAULT)
            .expect("axis drags have a guide");
        // Long enough to cross the whole view from the node's origin (the
        // visible diagonal both ways), thin, and along +X.
        assert!(guide.size.x >= viewport_px.length() * 2.0 - EPS);
        assert!((guide.size.y - GUIDE_PX).abs() < EPS);
        assert!(guide.rotation.abs() < EPS);
        // The line's long midline passes through the node's world origin.
        let mid_y = guide.position.y + guide.size.y * 0.5;
        assert!((mid_y - 100.0).abs() < EPS);

        // A free move has no axis, so no guide.
        let free = Drag::Move {
            node,
            original: scene.node(node).transform,
            grab_world: Vec2::ZERO,
        };
        assert!(axis_guide_sprite(&free, &scene, &cam, viewport_px, &Palette::DEFAULT).is_none());
    }
}
