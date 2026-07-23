//! atlas viewport interaction: the editor pan/zoom camera, the selection gizmo
//! (move arrows, center-pivot rotate, uniform and per-axis scale), and the
//! pure math each drag applies (M3 step 7). Everything here is headless and
//! unit-tested; main.rs only routes input into it.
//!
//! The model anchors a node at its top-left (photon rotates a sprite about its
//! position), but the gizmo deliberately edits about the sprite's CENTER: a
//! rotation or scale adjusts the translation too, so the center stays fixed -
//! what users expect from an editor. One drag still commits as one transform.

use glam::Vec2;
use helios::{Component, NodeId, Scene, Transform};
use photon::{Camera, Color, Sprite};

/// Selection accent (outline) - matches the UI's selected-row blue.
const OUTLINE: Color = Color::new(0.30, 0.55, 0.90, 1.0);
/// The engine-wide axis palette (also for future inspector x/y fields): X red,
/// Y green - and rotation, being about Z, gets Z's blue.
const AXIS_X: Color = Color::new(0.90, 0.32, 0.32, 1.0);
const AXIS_Y: Color = Color::new(0.35, 0.80, 0.35, 1.0);
const ROTATE_HANDLE: Color = Color::new(0.35, 0.55, 1.0, 1.0);
/// The uniform-scale corner handle, amber - distinct from every axis color.
const SCALE_HANDLE: Color = Color::new(1.0, 0.75, 0.25, 1.0);

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
    /// The world center of the sprite's quad - the pivot rotate and scale
    /// drags work about.
    pub center: Vec2,
    /// The sprite's local half-extent (component size / 2, unscaled) - what
    /// the center-pivot math needs to recompute the translation.
    pub half: Vec2,
    /// World top-left corner (the node's translation).
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

/// Which gizmo handle a press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoHit {
    /// The X move arrow: translate along the sprite's local X only.
    MoveX,
    /// The Y move arrow: translate along the sprite's local Y only.
    MoveY,
    /// The rotate handle: rotate about the center.
    Rotate,
    /// The corner handle: uniform scale about the center.
    ScaleUniform,
    /// The right-edge handle: scale X only, about the center.
    ScaleX,
    /// The bottom-edge handle: scale Y only, about the center.
    ScaleY,
}

/// Build the gizmo for `node`, or `None` if it has no Sprite component to
/// frame (e.g. the root selected in the tree).
pub fn gizmo(scene: &Scene, node: NodeId, zoom: f32) -> Option<Gizmo> {
    let sprite_size = scene
        .node(node)
        .components
        .iter()
        .map(|c| match c {
            Component::Sprite(s) => s.size,
        })
        .next()?;
    let affine = scene.world_transform(node);
    let (scale, angle, translation) = affine.to_scale_angle_translation();
    let size = scale * sprite_size;
    let axis_x = Vec2::from_angle(angle);
    let axis_y = axis_x.perp();
    let center = affine.transform_point2(sprite_size * 0.5);
    let arrow = ARROW_PX / zoom;

    Some(Gizmo {
        center,
        half: sprite_size * 0.5,
        anchor: translation,
        size,
        angle,
        axis_x,
        axis_y,
        rotate_center: center - axis_y * (size.y * 0.5 + ROTATE_OFFSET_PX / zoom),
        scale_corner: affine.transform_point2(sprite_size),
        scale_x: affine.transform_point2(Vec2::new(sprite_size.x, sprite_size.y * 0.5)),
        scale_y: affine.transform_point2(Vec2::new(sprite_size.x * 0.5, sprite_size.y)),
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

/// Which handle (if any) a world-space press lands on. Point handles win over
/// the arrow shafts, and the shafts over the sprite body (the caller falls
/// back to picking).
pub fn hit_gizmo(gizmo: &Gizmo, world: Vec2) -> Option<GizmoHit> {
    let r = gizmo.grab_radius;
    let hits = [
        (GizmoHit::Rotate, gizmo.rotate_center),
        (GizmoHit::ScaleUniform, gizmo.scale_corner),
        (GizmoHit::ScaleX, gizmo.scale_x),
        (GizmoHit::ScaleY, gizmo.scale_y),
        (GizmoHit::MoveX, gizmo.arrow_x_end),
        (GizmoHit::MoveY, gizmo.arrow_y_end),
    ];
    for (hit, at) in hits {
        if world.distance(at) <= r {
            return Some(hit);
        }
    }
    // The arrow shafts, as segments (a slightly tighter radius than the
    // endpoint squares so they do not swallow clicks near the sprite body).
    if segment_distance(world, gizmo.center, gizmo.arrow_x_end) <= r * 0.6 {
        return Some(GizmoHit::MoveX);
    }
    if segment_distance(world, gizmo.center, gizmo.arrow_y_end) <= r * 0.6 {
        return Some(GizmoHit::MoveY);
    }
    None
}

/// The gizmo's overlay sprites (drawn with a solid-white texture, tinted): the
/// selection outline, the X/Y move arrows, and the rotate and scale handles,
/// all sized in screen pixels so they stay constant under zoom.
pub fn gizmo_sprites(gizmo: &Gizmo, zoom: f32) -> Vec<Sprite> {
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
    vec![
        // The four edges of the (possibly rotated) selection rectangle.
        quad(at(Vec2::ZERO), Vec2::new(w, t), OUTLINE),
        quad(at(Vec2::ZERO), Vec2::new(t, h), OUTLINE),
        quad(at(Vec2::new(0.0, h - t)), Vec2::new(w, t), OUTLINE),
        quad(at(Vec2::new(w - t, 0.0)), Vec2::new(t, h), OUTLINE),
        // The move arrows: a shaft from the center along each local axis, with
        // an endpoint square. Axis colors: X red, Y green.
        quad(
            gizmo.center - gizmo.axis_y * (t * 0.5),
            Vec2::new(arrow, t),
            AXIS_X,
        ),
        handle(gizmo.arrow_x_end, AXIS_X),
        quad(
            gizmo.center - gizmo.axis_x * (t * 0.5),
            Vec2::new(t, arrow),
            AXIS_Y,
        ),
        handle(gizmo.arrow_y_end, AXIS_Y),
        // A stem from the top edge up to the rotate handle, then the handles:
        // rotate (blue), per-axis scale (axis colors), uniform scale (amber).
        quad(
            at(Vec2::new(w * 0.5 - t * 0.5, -ROTATE_OFFSET_PX / zoom)),
            Vec2::new(t, ROTATE_OFFSET_PX / zoom),
            OUTLINE,
        ),
        handle(gizmo.rotate_center, ROTATE_HANDLE),
        handle(gizmo.scale_x, AXIS_X),
        handle(gizmo.scale_y, AXIS_Y),
        handle(gizmo.scale_corner, SCALE_HANDLE),
    ]
}

/// The sprite-quad center in *parent* space for a local transform: where the
/// center-pivot drags hold the sprite still.
fn center_in_parent(t: Transform, half: Vec2) -> Vec2 {
    t.translation + Vec2::from_angle(t.rotation).rotate(t.scale * half)
}

/// The translation that puts the sprite-quad center at `center` (parent space)
/// for the given rotation and scale - the inverse of [`center_in_parent`].
fn translation_keeping_center(center: Vec2, rotation: f32, scale: Vec2, half: Vec2) -> Vec2 {
    center - Vec2::from_angle(rotation).rotate(scale * half)
}

/// An in-progress viewport drag. Each records the transform at press time and
/// applies its math from that original - a release commits one clean undo step.
/// Rotation and scale pivot about the sprite's center (the translation moves to
/// keep the center fixed), because that is what an editor user expects even
/// though the model anchors at the top-left.
#[derive(Debug, Clone, Copy)]
pub enum Drag {
    /// Moving the node body freely: `grab_world` is where the press landed.
    Move {
        node: NodeId,
        original: Transform,
        grab_world: Vec2,
    },
    /// Moving along one world-space axis direction only (a gizmo arrow).
    MoveAxis {
        node: NodeId,
        original: Transform,
        grab_world: Vec2,
        axis_world: Vec2,
    },
    /// Rotating about the sprite's center: `grab_angle` is the cursor's angle
    /// about `pivot` (the world center at press time).
    Rotate {
        node: NodeId,
        original: Transform,
        half: Vec2,
        pivot: Vec2,
        grab_angle: f32,
    },
    /// Uniformly scaling about the center: `grab_dist` is the cursor's
    /// distance from the pivot at press time.
    ScaleUniform {
        node: NodeId,
        original: Transform,
        half: Vec2,
        pivot: Vec2,
        grab_dist: f32,
    },
    /// Scaling one local axis about the center: the cursor's offset from the
    /// pivot is projected onto `axis_world`, and the ratio to the press-time
    /// projection scales that component.
    ScaleAxis {
        node: NodeId,
        original: Transform,
        half: Vec2,
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
            } => {
                // Only the component of the cursor delta along the axis moves
                // the node - the constrained translate of a gizmo arrow.
                let along = (world - grab_world).dot(axis_world);
                move_by(scene, node, original, axis_world * along)
            }
            Drag::Rotate {
                original,
                half,
                pivot,
                grab_angle,
                ..
            } => {
                // The change in the cursor's angle about the pivot is the same
                // in world and parent-local space (a parent rotation offsets
                // both by a constant), so it adds directly; the translation
                // then moves to hold the center still.
                let rotation = original.rotation + ((world - pivot).to_angle() - grab_angle);
                let center = center_in_parent(original, half);
                Transform {
                    rotation,
                    translation: translation_keeping_center(center, rotation, original.scale, half),
                    ..original
                }
            }
            Drag::ScaleUniform {
                original,
                half,
                pivot,
                grab_dist,
                ..
            } => {
                let k = (world.distance(pivot) / grab_dist.max(1.0e-3)).max(0.05);
                let scale = original.scale * k;
                let center = center_in_parent(original, half);
                Transform {
                    scale,
                    translation: translation_keeping_center(center, original.rotation, scale, half),
                    ..original
                }
            }
            Drag::ScaleAxis {
                original,
                half,
                pivot,
                axis_world,
                grab_proj,
                vertical,
                ..
            } => {
                let proj = (world - pivot).dot(axis_world);
                let k = (proj / grab_proj).max(0.05);
                let mut scale = original.scale;
                if vertical {
                    scale.y *= k;
                } else {
                    scale.x *= k;
                }
                let center = center_in_parent(original, half);
                Transform {
                    scale,
                    translation: translation_keeping_center(center, original.rotation, scale, half),
                    ..original
                }
            }
        }
    }
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

    /// The world center of `node`'s sprite quad, from its current transform.
    fn world_center(scene: &Scene, node: NodeId, half: Vec2) -> Vec2 {
        scene
            .world_transform(node)
            .transform_point2(half * 2.0 * 0.5)
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

        assert!((g.center - Vec2::new(130.0, 70.0)).length() < EPS);
        assert!((g.scale_corner - Vec2::new(160.0, 90.0)).length() < EPS);
        assert!((g.scale_x - Vec2::new(160.0, 70.0)).length() < EPS);
        assert!((g.scale_y - Vec2::new(130.0, 90.0)).length() < EPS);
        // Unrotated: the rotate handle floats above the top mid-edge, and the
        // arrows point along +X and +Y from the center.
        assert!((g.rotate_center - Vec2::new(130.0, 50.0 - 26.0)).length() < EPS);
        assert!((g.arrow_x_end - Vec2::new(130.0 + 56.0, 70.0)).length() < EPS);
        assert!((g.arrow_y_end - Vec2::new(130.0, 70.0 + 56.0)).length() < EPS);

        assert_eq!(hit_gizmo(&g, g.rotate_center), Some(GizmoHit::Rotate));
        assert_eq!(hit_gizmo(&g, g.scale_corner), Some(GizmoHit::ScaleUniform));
        assert_eq!(hit_gizmo(&g, g.scale_x), Some(GizmoHit::ScaleX));
        assert_eq!(hit_gizmo(&g, g.scale_y), Some(GizmoHit::ScaleY));
        assert_eq!(hit_gizmo(&g, g.arrow_x_end), Some(GizmoHit::MoveX));
        assert_eq!(hit_gizmo(&g, g.arrow_y_end), Some(GizmoHit::MoveY));
        // A point along the X arrow shaft (between center and tip) hits too -
        // chosen clear of the right-edge scale handle at +30, which sits on
        // the same ray and rightly wins where they cross (point handles beat
        // shafts).
        assert_eq!(
            hit_gizmo(&g, g.center + g.axis_x * 45.0),
            Some(GizmoHit::MoveX)
        );
        assert_eq!(hit_gizmo(&g, Vec2::new(0.0, 0.0)), None);
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
        };
        // A diagonal cursor move: only its X component applies.
        let moved = drag.apply(&scene, Vec2::new(50.0, 80.0));
        assert!((moved.translation - Vec2::new(40.0, 10.0)).length() < EPS);
    }

    #[test]
    fn rotation_pivots_about_the_sprite_center() {
        let (mut scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::new(40.0, 20.0));
        let half = Vec2::new(20.0, 10.0);
        let center_before = world_center(&scene, node, half);
        let original = scene.node(node).transform;

        let drag = Drag::Rotate {
            node,
            original,
            half,
            pivot: center_before,
            grab_angle: 0.0,
        };
        // Drag to below the pivot: +90 degrees (Y-down world).
        let rotated = drag.apply(&scene, center_before + Vec2::new(0.0, 50.0));
        assert!((rotated.rotation - std::f32::consts::FRAC_PI_2).abs() < EPS);

        // Apply it and verify the sprite's center did not move - the whole
        // point of pivoting about the center rather than the anchor.
        scene.node_mut(node).transform = rotated;
        let center_after = world_center(&scene, node, half);
        assert!(
            (center_before - center_after).length() < EPS,
            "center moved: {center_before} -> {center_after}"
        );
    }

    #[test]
    fn uniform_scale_pivots_about_the_center() {
        let (mut scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::new(40.0, 20.0));
        let half = Vec2::new(20.0, 10.0);
        let center_before = world_center(&scene, node, half);
        let original = scene.node(node).transform;

        let drag = Drag::ScaleUniform {
            node,
            original,
            half,
            pivot: center_before,
            grab_dist: 20.0,
        };
        let scaled = drag.apply(&scene, center_before + Vec2::new(50.0, 0.0));
        assert!((scaled.scale - Vec2::splat(2.5)).length() < EPS);

        scene.node_mut(node).transform = scaled;
        let center_after = world_center(&scene, node, half);
        assert!((center_before - center_after).length() < EPS);
    }

    #[test]
    fn axis_scale_changes_one_component_and_keeps_the_center() {
        let (mut scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::new(40.0, 20.0));
        let half = Vec2::new(20.0, 10.0);
        let center = world_center(&scene, node, half);
        let original = scene.node(node).transform;

        // Grab the right-edge handle (20 world units from the center along X)
        // and drag it out to 40: X doubles, Y untouched.
        let drag = Drag::ScaleAxis {
            node,
            original,
            half,
            pivot: center,
            axis_world: Vec2::X,
            grab_proj: 20.0,
            vertical: false,
        };
        let scaled = drag.apply(&scene, center + Vec2::new(40.0, 0.0));
        assert!((scaled.scale - Vec2::new(2.0, 1.0)).length() < EPS);

        scene.node_mut(node).transform = scaled;
        assert!((world_center(&scene, node, half) - center).length() < EPS);
    }
}
