//! photon - 2D drawing API (sprites, shapes, text, render targets), and the
//! engine's renderer. It touches wgpu, sharing its GPU context (via `aether`)
//! with the editor's GUI backend rather than owning wgpu exclusively (ADR 0018).

mod camera;
mod color;
mod error;
mod renderer;
mod scene_target;
pub mod shapes;
mod sprite;
mod texture;

pub use camera::Camera;
pub use color::Color;
pub use error::RendererError;
pub use renderer::Renderer;
pub use scene_target::SceneTarget;
pub use sprite::{Sprite, pack_sprite_instances};
pub use texture::Texture;
