//! photon sprite: a textured quad in world space, plus its packed per-instance GPU form.

use crate::color::Color;
use glam::Vec2;

/// A textured quad placed in world space (ADR 0012). All sprites drawn in one
/// call share a single texture; the quad is transformed by a 2D affine built
/// from `position`, `size`, and `rotation`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    /// World position of the sprite's top-left corner (before rotation).
    pub position: Vec2,
    /// Size in pixels (world units).
    pub size: Vec2,
    /// Rotation in radians, clockwise (Y is down), about the top-left corner.
    pub rotation: f32,
    /// Sub-rectangle of the texture to sample: `(min_u, min_v, max_u, max_v)` in
    /// `0..1`. Defaults to the whole texture.
    pub uv_rect: [f32; 4],
    /// Color multiplied into the sampled texel; `Color::WHITE` draws the texture
    /// unchanged.
    pub tint: Color,
}

impl Sprite {
    /// A sprite covering `size` pixels at `position`, drawing the whole texture
    /// untinted and unrotated.
    pub fn new(position: Vec2, size: Vec2) -> Self {
        Self {
            position,
            size,
            rotation: 0.0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            tint: Color::WHITE,
        }
    }

    /// Pack into the GPU instance record.
    pub(crate) fn to_raw(self) -> RawInstance {
        let (sin, cos) = self.rotation.sin_cos();
        let (sx, sy) = (self.size.x, self.size.y);
        RawInstance {
            // Columns of the 2D affine (mat3x2): the linear 2x2 part is
            // rotation * scale, the third column is the translation. This maps
            // the unit quad in the shader onto the sprite's world rectangle.
            transform: [
                cos * sx,
                sin * sx,
                -sin * sy,
                cos * sy,
                self.position.x,
                self.position.y,
            ],
            uv_rect: self.uv_rect,
            tint: self.tint.to_array(),
        }
    }
}

/// Per-instance data uploaded to the GPU, one record per sprite. `#[repr(C)]`
/// so the byte layout matches the vertex attributes declared in `layout` and
/// the `Instance` struct in sprite.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct RawInstance {
    transform: [f32; 6],
    uv_rect: [f32; 4],
    tint: [f32; 4],
}

impl RawInstance {
    /// The instance-step vertex-buffer layout matching sprite.wgsl.
    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x2, // transform column 0
            1 => Float32x2, // transform column 1
            2 => Float32x2, // transform translation
            3 => Float32x4, // uv_rect
            4 => Float32x4, // tint
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<RawInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        }
    }
}
