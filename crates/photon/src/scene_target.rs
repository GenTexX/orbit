//! photon scene target: a sampleable offscreen render target for the editor's
//! viewport (ADR 0018). Unlike [`Renderer::render_to_image`](crate::Renderer::render_to_image),
//! nothing is read back to the CPU - a GUI backend on the same device samples
//! this texture directly.

/// An offscreen texture the scene renders into, sized once and reused every
/// frame. Read [`view`](Self::view) to hand the texture to a GPU-aware
/// compositor (e.g. aurora-wgpu); photon itself never names a GUI type.
pub struct SceneTarget {
    #[allow(
        dead_code,
        reason = "kept alive alongside the view, which borrows from it"
    )]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl SceneTarget {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene target"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // RENDER_ATTACHMENT so photon can draw into it; TEXTURE_BINDING (not
            // COPY_SRC) so a compositor can sample it directly - no CPU
            // round-trip, unlike `render_to_image`'s readback target.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width,
            height,
        }
    }

    /// The texture view a GPU-aware compositor samples from.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}
