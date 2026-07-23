//! helios rendering: turn a Scene into photon sprite instances. The engine owns
//! scene-to-sprites (it is a rendering concern, per CONTEXT's Engine); this is
//! still CPU-only - it produces the list, photon draws it.

use photon::{Color, Sprite};

use crate::component::{Component, SpriteComponent};
use crate::scene::{NodeId, Scene};

impl Scene {
    /// Collect this scene's drawable sprites in draw order (a pre-order walk from
    /// the root, so children draw over their parents), each placed by its node's
    /// world transform. Hand the result to photon to render.
    pub fn sprites(&self) -> Vec<Sprite> {
        let mut out = Vec::new();
        self.collect_sprites(self.root(), &mut out);
        out
    }

    fn collect_sprites(&self, id: NodeId, out: &mut Vec<Sprite>) {
        for component in &self.node(id).components {
            // Exhaustive so a new component kind (e.g. Camera) forces a decision
            // here rather than being silently non-drawable.
            match component {
                Component::Sprite(sprite) => out.push(self.build_sprite(id, sprite)),
            }
        }
        for &child in self.children(id) {
            self.collect_sprites(child, out);
        }
    }

    /// Turn a node's Sprite component into a photon `Sprite` placed by the node's
    /// world transform. The node's origin is the sprite's top-left (photon's
    /// anchor); the sprite's own `size` is scaled by the world scale.
    ///
    /// The world affine is decomposed into scale/rotation/translation, which is
    /// exact for translate/rotate/uniform-scale - everything the editor does. A
    /// sheared affine (a non-uniformly-scaled parent with a rotated child) has no
    /// exact photon `Sprite`; it is approximated. If that ever matters, photon's
    /// `Sprite` can grow a raw-affine constructor (ADR 0012's mat3x2 already
    /// carries a full affine) without changing callers here.
    fn build_sprite(&self, id: NodeId, sprite: &SpriteComponent) -> Sprite {
        let (scale, angle, translation) = self.world_transform(id).to_scale_angle_translation();
        let [r, g, b, a] = sprite.tint;
        let mut placed = Sprite::new(translation, scale * sprite.size);
        placed.rotation = angle;
        placed.tint = Color::new(r, g, b, a);
        placed
    }
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
        assert!((sprites[0].position - Vec2::new(100.0, 50.0)).length() < 1e-4);
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
        // Inside the sprite's world rect (x 10..50, y 10..40) is red; outside is
        // the clear color - the scene reached pixels at the right place.
        assert_eq!(px(30, 20), [255, 0, 0, 255]);
        assert_eq!(px(80, 80), [0, 0, 0, 255]);
    }
}
