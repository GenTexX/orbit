//! aurora-wgpu image registry: backend-owned textures Aurora's draw list
//! references by opaque [`ImageHandle`] (ADR 0018), e.g. the engine's viewport.

use aurora::ImageHandle;
use slotmap::SlotMap;

struct RegisteredImage {
    bind_group: wgpu::BindGroup,
    /// Kept alive for registry-owned textures (e.g. rasterized icons); `None`
    /// when the caller owns the texture (e.g. the viewport's render target).
    _texture: Option<wgpu::Texture>,
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
            // Trilinear: registry-owned images (icons, thumbnails) carry a
            // mipmap chain (see make_rgba), so shrinking a 64px icon to a 16px
            // toolbar slot samples the right level instead of aliasing.
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
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
        self.images.insert(RegisteredImage {
            bind_group,
            _texture: None,
        })
    }

    /// Register a registry-owned image from raw RGBA8 bytes (`width * height *
    /// 4`, row-major) - e.g. a rasterized icon. The texture is created, filled,
    /// and kept alive here, so the caller need not manage it.
    pub fn register_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> ImageHandle {
        let (texture, bind_group) = self.make_rgba(device, queue, layout, rgba, width, height);
        self.images.insert(RegisteredImage {
            bind_group,
            _texture: Some(texture),
        })
    }

    /// Replace a registry-owned image's pixels (e.g. the color picker's SV
    /// square as the hue changes), keeping its handle valid. Returns `false`
    /// on an unknown handle.
    // Mirrors register_rgba's GPU handles plus the target handle; a params
    // struct would not read more clearly.
    #[allow(clippy::too_many_arguments)]
    pub fn update_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        handle: ImageHandle,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> bool {
        let (texture, bind_group) = self.make_rgba(device, queue, layout, rgba, width, height);
        match self.images.get_mut(handle) {
            Some(image) => {
                image.bind_group = bind_group;
                image._texture = Some(texture);
                true
            }
            None => false,
        }
    }

    /// Create a texture from RGBA8 bytes and its bind group (shared by
    /// register/update).
    fn make_rgba(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::BindGroup) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        // A full mipmap chain (down to 1x1) so minified draws (a 64px icon in a
        // 16px slot) pick a pre-filtered level rather than point-aliasing.
        let mip_level_count = 1 + width.max(height).max(1).ilog2();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("registered image"),
            size,
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // Upload level 0, then box-downsample each successive level on the CPU
        // and upload it. (The icon art is white with an alpha coverage mask, so
        // averaging the bytes is correct; for photos it is the usual good-enough
        // sRGB box filter.)
        let mut level = rgba.to_vec();
        let (mut lw, mut lh) = (width, height);
        for mip in 0..mip_level_count {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &level,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(lw * 4),
                    rows_per_image: Some(lh),
                },
                wgpu::Extent3d {
                    width: lw,
                    height: lh,
                    depth_or_array_layers: 1,
                },
            );
            if mip + 1 < mip_level_count {
                (level, lw, lh) = downsample_2x2(&level, lw, lh);
            }
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.build_bind_group(device, layout, &view);
        (texture, bind_group)
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

/// Box-downsample an RGBA8 image to half size (each axis halved, floored at 1),
/// averaging each 2x2 block per channel. Edge blocks on an odd dimension average
/// only the texels that exist.
///
/// The source is sRGB-encoded (the texture is `Rgba8UnormSrgb`), so the color
/// channels are decoded to linear light before averaging and re-encoded after.
/// Averaging the raw sRGB bytes would darken every mip level - a box filter of
/// black and white should read as mid-gray (~188), not the too-dark 128 a naive
/// byte average gives. Alpha is already linear, so it is averaged directly.
fn downsample_2x2(src: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let nw = (width / 2).max(1);
    let nh = (height / 2).max(1);
    // Decode the 256 possible sRGB byte values to linear once, then reuse.
    let to_linear: [f32; 256] = std::array::from_fn(srgb_to_linear);
    let mut dst = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let mut sum = 0.0f32;
                let mut n = 0u32;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (px, py) = (x * 2 + dx, y * 2 + dy);
                        if px < width && py < height {
                            let byte = src[((py * width + px) * 4 + c) as usize];
                            // Alpha (channel 3) is linear; color channels are sRGB.
                            sum += if c == 3 {
                                byte as f32 / 255.0
                            } else {
                                to_linear[byte as usize]
                            };
                            n += 1;
                        }
                    }
                }
                let avg = sum / n as f32;
                let out = if c == 3 {
                    (avg * 255.0).round()
                } else {
                    linear_to_srgb(avg)
                };
                dst[((y * nw + x) * 4 + c) as usize] = out as u8;
            }
        }
    }
    (dst, nw, nh)
}

/// Decode one sRGB-encoded byte (0..=255) to linear light in [0, 1].
fn srgb_to_linear(byte: usize) -> f32 {
    let c = byte as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Encode linear light in [0, 1] back to an sRGB byte value (0..=255, as f32).
fn linear_to_srgb(l: f32) -> f32 {
    let c = if l <= 0.0031308 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_halves_and_averages() {
        // A 2x2 image with channel-0 values 0,100,200,255 averaged in LINEAR
        // light -> 175, not the too-dark 138 a raw sRGB byte average would give.
        // The other (constant) color channels round-trip through linear; alpha
        // (channel 3) averages straight.
        let src = vec![
            0, 10, 20, 30, //
            100, 10, 20, 30, //
            200, 10, 20, 30, //
            255, 10, 20, 30,
        ];
        let (dst, w, h) = downsample_2x2(&src, 2, 2);
        assert_eq!((w, h), (1, 1));
        assert_eq!(dst, vec![175u8, 10, 20, 30]);
    }

    #[test]
    fn downsample_handles_odd_dimensions() {
        // 3x1 -> 1x1: the lone output block averages only the two texels that
        // exist in its 2x2 footprint (x=0,1; x=2 falls in the next, absent block).
        // 0 and 40 averaged in linear light re-encode to 26.
        let src = vec![0, 0, 0, 0, 40, 0, 0, 0, 200, 0, 0, 0];
        let (dst, w, h) = downsample_2x2(&src, 3, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(dst[0], 26);
    }
}
