//! aurora-wgpu image registry: backend-owned textures Aurora's draw list
//! references by opaque [`ImageHandle`] (ADR 0018), e.g. the engine's viewport.

use aurora::ImageHandle;
use slotmap::SlotMap;

struct RegisteredImage {
    bind_group: wgpu::BindGroup,
}

/// The registered images, each holding a bind group (texture + sampler) built
/// against the shared `texture_layout` (the same layout the glyph atlas uses).
///
/// Milestone 3 only ever registers one image (the scene viewport) and never
/// removes it. A recreated texture (e.g. the viewport resized) keeps its
/// handle via [`update`](Self::update), so widgets referencing it stay valid.
pub(crate) struct ImageRegistry {
    images: SlotMap<ImageHandle, RegisteredImage>,
    sampler: wgpu::Sampler,
}

impl ImageRegistry {
    pub fn new(device: &wgpu::Device) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            images: SlotMap::with_key(),
            sampler,
        }
    }

    fn build_bind_group(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Register `view` for sampling against `layout` (the shared
    /// texture+sampler bind group layout), returning the handle Aurora's draw
    /// list will reference it by.
    pub fn register(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
    ) -> ImageHandle {
        let bind_group = self.build_bind_group(device, layout, view);
        self.images.insert(RegisteredImage { bind_group })
    }

    /// Point an existing handle at a new texture view (e.g. the viewport's
    /// target recreated at a new size), keeping the handle - and every widget
    /// referencing it - valid. Returns `false` on an unknown handle.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        handle: ImageHandle,
        view: &wgpu::TextureView,
    ) -> bool {
        let bind_group = self.build_bind_group(device, layout, view);
        match self.images.get_mut(handle) {
            Some(image) => {
                image.bind_group = bind_group;
                true
            }
            None => false,
        }
    }

    /// The bind group for a registered image, or `None` for an unknown or
    /// stale handle (skipped rather than panicking - a draw list can outlive a
    /// resize that re-registered its image under a new handle for one frame).
    pub fn bind_group(&self, handle: ImageHandle) -> Option<&wgpu::BindGroup> {
        self.images.get(handle).map(|i| &i.bind_group)
    }
}
