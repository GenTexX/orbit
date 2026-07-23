//! helios transform: a node's 2D position, rotation, and scale, and the affine it composes to.

use glam::{Affine2, Vec2};

/// A node's local spatial transform - where it sits relative to its parent.
///
/// The engine's world space is Y-down, top-left, in pixels (ADR 0012), and this
/// composes into the same 2D affine photon's sprite instances already consume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Position relative to the parent, in pixels.
    pub translation: Vec2,
    /// Rotation in radians.
    pub rotation: f32,
    /// Scale factor per axis (1.0 is unscaled).
    pub scale: Vec2,
}

impl Transform {
    /// The identity transform: origin, no rotation, unit scale.
    pub const IDENTITY: Transform = Transform {
        translation: Vec2::ZERO,
        rotation: 0.0,
        scale: Vec2::ONE,
    };

    /// A transform at `translation`, unrotated and unscaled.
    pub fn from_translation(translation: Vec2) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    /// The local affine this transform represents: scale, then rotate, then
    /// translate. Composing a node's affine with its ancestors' yields its world
    /// transform (see [`Scene::world_transform`](crate::Scene::world_transform)).
    pub fn to_affine(self) -> Affine2 {
        Affine2::from_scale_angle_translation(self.scale, self.rotation, self.translation)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}
