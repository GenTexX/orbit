//! atlas viewport interaction: the editor pan/zoom camera, selection gizmos
//! (outline, rotate and scale handles), and the pure math each drag applies
//! (M3 step 7). Everything here is headless and unit-tested; main.rs only
//! routes input into it.

use glam::Vec2;
use helios::{Component, NodeId, Scene, Transform};
use photon::{Camera, Color, Sprite};

/// Selection accent (outline and hover) - matches the UI's selected-row blue.
const OUTLINE: Color = Color::new(0.30, 0.55, 0.90, 1.0);
/// The rotate handle, blue: rotation is about the Z axis, and Z is blue in the
/// engine-wide axis palette (X red, Y green, Z blue - used by future
/// per-axis gizmos and inspector fields too).
const ROTATE_HANDLE: Color = Color::new(0.35, 0.55, 1.0, 1.0);
/// The scale handle, amber - distinct from every axis color.
const SCALE_HANDLE: Color = Color::new(1.0, 0.75, 0.25, 1.0);

/// Outline thickness, handle half-size, and grab radius, in *screen* pixels
/// (divided by zoom at use, so gizmos keep their size as the view zooms).
/// The grab radius is deliberately larger than the visual handle - the same
/// lesson as the too-thin splitter bars.
const OUTLINE_PX: f32 = 2.0;
const HANDLE_HALF_PX: f32 = 5.0;
const GRAB_RADIUS_PX: f32 = 10.0;
/// How far above the sprite's top edge the rotate handle floats, in screen px.
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

/// The selected node's gizmo, in world space: the sprite's oriented corners
/// (for the outline) and the two handle anchor points.
pub struct Gizmo {
    /// The node's world translation - the anchor rotation and scale work about
    /// (photon rotates a sprite about its top-left corner).
    pub anchor: Vec2,
    /// The world-space quad size (sprite size times world scale).
    size: Vec2,
    /// The world rotation angle in radians.
    angle: f32,
    /// Center of the rotate handle.
    pub rotate_center: Vec2,
    /// Center of the scale handle (the sprite's far corner).
    pub scale_center: Vec2,
    /// Handle grab radius in world units.
    pub grab_radius: f32,
}

/// Which gizmo handle a press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoHit {
    Rotate,
    Scale,
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
    let (scale, angle, translation) = scene.world_transform(node).to_scale_angle_translation();
    let size = scale * sprite_size;
    let rot = Vec2::from_angle(angle);

    Some(Gizmo {
        anchor: translation,
        size,
        angle,
        rotate_center: translation + rot.rotate(Vec2::new(size.x * 0.5, -ROTATE_OFFSET_PX / zoom)),
        scale_center: translation + rot.rotate(size),
        grab_radius: GRAB_RADIUS_PX / zoom,
    })
}

/// Which handle (if any) a world-space press lands on.
pub fn hit_gizmo(gizmo: &Gizmo, world: Vec2) -> Option<GizmoHit> {
    if world.distance(gizmo.rotate_center) <= gizmo.grab_radius {
        Some(GizmoHit::Rotate)
    } else if world.distance(gizmo.scale_center) <= gizmo.grab_radius {
        Some(GizmoHit::Scale)
    } else {
        None
    }
}

/// The gizmo's overlay sprites (drawn with a solid-white texture, tinted): a
/// rotated outline around the selection plus the two handles, all sized in
/// screen pixels so they stay constant under zoom.
pub fn gizmo_sprites(gizmo: &Gizmo, zoom: f32) -> Vec<Sprite> {
    let t = OUTLINE_PX / zoom;
    let (w, h) = (gizmo.size.x, gizmo.size.y);
    let rot = Vec2::from_angle(gizmo.angle);
    let at = |local: Vec2| gizmo.anchor + rot.rotate(local);

    let edge = |pos: Vec2, size: Vec2| {
        let mut s = Sprite::new(pos, size);
        s.rotation = gizmo.angle;
        s.tint = OUTLINE;
        s
    };
    let handle = |center: Vec2, tint: Color| {
        let half = HANDLE_HALF_PX / zoom;
        let mut s = Sprite::new(center - Vec2::splat(half), Vec2::splat(half * 2.0));
        s.tint = tint;
        s
    };

    vec![
        // The four edges of the (possibly rotated) selection rectangle.
        edge(at(Vec2::ZERO), Vec2::new(w, t)),
        edge(at(Vec2::ZERO), Vec2::new(t, h)),
        edge(at(Vec2::new(0.0, h - t)), Vec2::new(w, t)),
        edge(at(Vec2::new(w - t, 0.0)), Vec2::new(t, h)),
        // A stem from the top edge up to the rotate handle, then the handles.
        edge(
            at(Vec2::new(w * 0.5 - t * 0.5, -ROTATE_OFFSET_PX / zoom)),
            Vec2::new(t, ROTATE_OFFSET_PX / zoom),
        ),
        handle(gizmo.rotate_center, ROTATE_HANDLE),
        handle(gizmo.scale_center, SCALE_HANDLE),
    ]
}

/// An in-progress viewport drag. Each records the transform at press time and
/// applies its math from that original - a release commits one clean undo step.
#[derive(Debug, Clone, Copy)]
pub enum Drag {
    /// Moving the node body: `grab_world` is where the press landed.
    Move {
        node: NodeId,
        original: Transform,
        grab_world: Vec2,
    },
    /// Rotating about the node's anchor: `grab_angle` is the cursor's angle
    /// about the anchor at press time.
    Rotate {
        node: NodeId,
        original: Transform,
        anchor: Vec2,
        grab_angle: f32,
    },
    /// Uniformly scaling about the anchor: `grab_dist` is the cursor's
    /// distance from the anchor at press time.
    Scale {
        node: NodeId,
        original: Transform,
        anchor: Vec2,
        grab_dist: f32,
    },
}

impl Drag {
    /// The node being dragged and its transform at press time.
    pub fn target(&self) -> (NodeId, Transform) {
        match *self {
            Drag::Move { node, original, .. }
            | Drag::Rotate { node, original, .. }
            | Drag::Scale { node, original, .. } => (node, original),
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
            } => {
                // The cursor moved in world space; the node's translation is in
                // its parent's local space, so map the delta through the
                // inverse of the parent's world transform (identity for a root
                // child). A vector transform: rotation and scale, no offset.
                let world_delta = world - grab_world;
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
            Drag::Rotate {
                original,
                anchor,
                grab_angle,
                ..
            } => {
                // The change in the cursor's angle about the anchor is the same
                // in world and parent-local space (a parent rotation is a
                // constant offset on both), so it adds directly.
                let angle = (world - anchor).to_angle();
                Transform {
                    rotation: original.rotation + (angle - grab_angle),
                    ..original
                }
            }
            Drag::Scale {
                original,
                anchor,
                grab_dist,
                ..
            } => {
                // Uniform scale: the ratio of cursor distances from the anchor.
                let dist = world.distance(anchor).max(1.0e-3);
                let s = dist / grab_dist.max(1.0e-3);
                Transform {
                    scale: original.scale * s,
                    ..original
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios::{Node, SpriteComponent};

    const EPS: f32 = 1.0e-4;

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

        assert!((g.anchor - Vec2::new(100.0, 50.0)).length() < EPS);
        assert!((g.scale_center - Vec2::new(160.0, 90.0)).length() < EPS);
        // Unrotated: the rotate handle floats straight above the top mid-edge.
        assert!((g.rotate_center - Vec2::new(130.0, 50.0 - 26.0)).length() < EPS);

        assert_eq!(hit_gizmo(&g, g.rotate_center), Some(GizmoHit::Rotate));
        assert_eq!(hit_gizmo(&g, g.scale_center), Some(GizmoHit::Scale));
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
    fn rotate_and_scale_drags_apply_about_the_anchor() {
        let (scene, node) = scene_with_sprite(Vec2::new(100.0, 100.0), Vec2::new(40.0, 40.0));
        let original = scene.node(node).transform;
        let anchor = Vec2::new(100.0, 100.0);

        // Grab to the right of the anchor, drag to below it: +90 degrees
        // (Y-down world, angles grow clockwise on screen).
        let drag = Drag::Rotate {
            node,
            original,
            anchor,
            grab_angle: 0.0,
        };
        let rotated = drag.apply(&scene, anchor + Vec2::new(0.0, 50.0));
        assert!((rotated.rotation - std::f32::consts::FRAC_PI_2).abs() < EPS);

        // Grab at distance 20, drag to distance 50: uniform scale 2.5x.
        let drag = Drag::Scale {
            node,
            original,
            anchor,
            grab_dist: 20.0,
        };
        let scaled = drag.apply(&scene, anchor + Vec2::new(50.0, 0.0));
        assert!((scaled.scale - Vec2::splat(2.5)).length() < EPS);
    }
}
