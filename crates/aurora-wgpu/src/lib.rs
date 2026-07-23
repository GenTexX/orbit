//! aurora-wgpu - renders aurora draw lists with wgpu: quad batching, glyph atlas, scissoring.

mod atlas;
mod error;
mod image;
mod renderer;

pub use error::RenderError;
pub use renderer::Renderer;
