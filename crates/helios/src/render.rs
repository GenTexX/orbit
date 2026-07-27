//! helios rendering: turn a Scene into photon sprite instances. The engine owns
//! scene-to-sprites (it is a rendering concern, per CONTEXT's Engine); this is
//! still CPU-only - it produces the list, photon draws it.

use glam::{Affine2, Vec2};
use photon::{Color, Sprite};

use crate::component::{Component, SpriteComponent};
use crate::scene::{NodeId, Scene};

/// One drawable sprite from the scene: the node that owns it, the texture path
/// it names (for per-texture batching), and its placed photon [`Sprite`].
#[derive(Debug, Clone)]
pub struct SpriteDraw {
    pub node: NodeId,
    pub texture: String,
    pub sprite: Sprite,
}

impl Scene {
    /// Collect this scene's drawable sprites in draw order (a pre-order walk from
    /// the root, so children draw over their parents), each placed by its node's
    /// world transform. Hand the result to photon to render.
    pub fn sprites(&self) -> Vec<Sprite> {
        self.sprite_draws().into_iter().map(|d| d.sprite).collect()
    }

    /// Like [`sprites`](Self::sprites), but each sprite paired with the node
    /// that owns it - the mapping GPU picking needs: photon returns an index
    /// into the sprite list, and this same walk (same order) says which node
    /// that index belongs to.
    pub fn sprite_entries(&self) -> Vec<(NodeId, Sprite)> {
        self.sprite_draws()
            .into_iter()
            .map(|d| (d.node, d.sprite))
            .collect()
    }

    /// Every drawable sprite in draw order, each with its owning node and the
    /// texture path it names - what a renderer needs to batch by texture
    /// (consecutive same-texture sprites) while keeping painter's order.
    pub fn sprite_draws(&self) -> Vec<SpriteDraw> {
        let mut out = Vec::new();
        self.collect_sprites(self.root(), Affine2::IDENTITY, &mut out);
        out
    }

    /// Walk the subtree in pre-order, threading each node's world transform down
    /// as `parent_world` and composing the local once per node - so the whole
    /// extraction is O(nodes), not O(nodes x depth) as it would be if each sprite
    /// re-derived its world transform by walking back up to the root.
    fn collect_sprites(&self, id: NodeId, parent_world: Affine2, out: &mut Vec<SpriteDraw>) {
        // A hidden node hides its whole subtree: skip its sprites and children.
        let node = self.node(id);
        if !node.visible {
            return;
        }
        let world = parent_world * node.transform.to_affine();
        for component in &node.components {
            // Exhaustive so a new component kind (e.g. Camera) forces a decision
            // here rather than being silently non-drawable.
            match component {
                Component::Sprite(sprite) => out.push(SpriteDraw {
                    node: id,
                    texture: sprite.texture.clone(),
                    sprite: build_sprite(world, sprite),
                }),
            }
        }
        for &child in self.children(id) {
            self.collect_sprites(child, world, out);
        }
    }
}

/// Place a node's Sprite component in the world at `world` (its node's world
/// affine). The node's origin is the sprite's CENTER (ADR 0019); photon's
/// `Sprite` anchors at its top-left and rotates about it, so the corner sits
/// half a rotated, scaled size up-left of the origin - this is the one place
/// that offset lives.
///
/// The world affine is decomposed into scale/rotation/translation, which is
/// exact for translate/rotate/uniform-scale - everything the editor does. A
/// sheared affine (a non-uniformly-scaled parent with a rotated child) has no
/// exact photon `Sprite`; it is approximated. If that ever matters, photon's
/// `Sprite` can grow a raw-affine constructor (ADR 0012's mat3x2 already carries
/// a full affine) without changing callers here.
fn build_sprite(world: Affine2, sprite: &SpriteComponent) -> Sprite {
    let (scale, angle, translation) = world.to_scale_angle_translation();
    let size = scale * sprite.size;
    let position = translation - Vec2::from_angle(angle).rotate(size * 0.5);
    let [r, g, b, a] = sprite.tint;
    let mut placed = Sprite::new(position, size);
    placed.rotation = angle;
    placed.tint = Color::new(r, g, b, a);
    placed
}

#[cfg(test)]
mod tests {
    use crate::component::{Component, SpriteComponent};
    use crate::scene::{Node, Scene};
    use crate::transform::Transform;
    use glam::Vec2;

    fn sprite_node(size: Vec2, tint: [f32; 4]) -> Node {
        let mut node = Node::new("sprite");
        node.components.push(Component::Sprite(SpriteComponent {
            texture: String::new(),
            tint,
            size,
        }));
        node
    }

    #[test]
    fn a_sprite_node_becomes_a_placed_sprite() {
        let mut scene = Scene::new("root");
        let root = scene.root();
        let mut node = sprite_node(Vec2::new(32.0, 48.0), [1.0, 0.0, 0.0, 1.0]);
        node.transform = Transform::from_translation(Vec2::new(100.0, 50.0));
        scene.add_child(root, node);

        let sprites = scene.sprites();
        assert_eq!(sprites.len(), 1);
        // The node's origin (100, 50) is the sprite's CENTER (ADR 0019), so the
        // photon quad's top-left sits half the size up-left of it.
        assert!((sprites[0].position - Vec2::new(84.0, 26.0)).length() < 1e-4);
        assert!((sprites[0].size - Vec2::new(32.0, 48.0)).length() < 1e-4);
        assert_eq!(sprites[0].tint, photon::Color::new(1.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn a_parent_scale_scales_the_childs_sprite() {
        let mut scene = Scene::new("root");
        let root = scene.root();
        scene.node_mut(root).transform = Transform {
            scale: Vec2::splat(2.0),
            ..Transform::IDENTITY
        };
        scene.add_child(root, sprite_node(Vec2::new(10.0, 10.0), [1.0; 4]));

        let sprites = scene.sprites();
        // The parent's 2x scale doubles the child sprite's size.
        assert!((sprites[0].size - Vec2::new(20.0, 20.0)).length() < 1e-3);
    }

    #[test]
    fn nodes_without_a_sprite_produce_nothing() {
        let mut scene = Scene::new("root"); // the root has no components
        let root = scene.root();
        scene.add_child(root, Node::new("empty"));
        assert!(scene.sprites().is_empty());
    }

    #[test]
    fn a_hidden_node_hides_itself_and_its_subtree() {
        let mut scene = Scene::new("root");
        let root = scene.root();
        let parent = scene.add_child(root, sprite_node(Vec2::splat(10.0), [1.0; 4]));
        scene.add_child(parent, sprite_node(Vec2::splat(10.0), [1.0; 4]));
        // Both the parent's and the child's sprites draw while visible.
        assert_eq!(scene.sprites().len(), 2);

        // Hiding the parent drops its sprite and its child's too (whole subtree).
        scene.node_mut(parent).visible = false;
        assert!(scene.sprites().is_empty());
    }

    #[test]
    #[ignore = "requires a GPU adapter; run locally with --ignored"]
    fn a_scene_renders_its_sprite_to_pixels() {
        use photon::{Camera, Color, Renderer};

        let mut scene = Scene::new("root");
        let root = scene.root();
        let mut node = sprite_node(Vec2::new(40.0, 30.0), [1.0, 0.0, 0.0, 1.0]);
        node.transform = Transform::from_translation(Vec2::new(10.0, 10.0));
        scene.add_child(root, node);

        let renderer = Renderer::headless().expect("headless renderer");
        let white = renderer.create_texture(&[255, 255, 255, 255], 1, 1);
        let (w, h) = (100u32, 100u32);
        let camera = Camera::new(Vec2::ZERO, Vec2::new(w as f32, h as f32));
        let image = renderer
            .render_to_image((w, h), Color::BLACK, &camera, &white, &scene.sprites())
            .expect("render");

        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [image[i], image[i + 1], image[i + 2], image[i + 3]]
        };
        // Centered on (10, 10) with size (40, 30), the quad spans x -10..30,
        // y -5..25 (ADR 0019): its on-screen part is red, outside is the clear
        // color - the scene reached pixels at the right place.
        assert_eq!(px(15, 15), [255, 0, 0, 255]);
        assert_eq!(px(29, 24), [255, 0, 0, 255]);
        assert_eq!(px(35, 15), [0, 0, 0, 255]);
        assert_eq!(px(80, 80), [0, 0, 0, 255]);
    }
}
