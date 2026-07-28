//! aurora-wgpu image registry: backend-owned textures Aurora's draw list
//! references by opaque [`ImageHandle`] (ADR 0018), e.g. the engine's viewport.

use aurora::ImageHandle;
use slotmap::SlotMap;

struct RegisteredImage {
    bind_group: wgpu::BindGroup,
    /// Kept alive for registry-owned textures (e.g. rasterized icons); `None`
    /// when the caller owns the texture (e.g. the viewport's render target).
    texture: Option<wgpu::Texture>,
    /// The registry-owned texture's shape, so [`ImageRegistry::update_rgba`] can
    /// recognise a same-size refill and write into the existing texture instead
    /// of building a new one. `None` when the caller owns the texture.
    shape: Option<TextureShape>,
}

/// A registry-owned texture's dimensions and mip-level count.
#[derive(Clone, Copy, PartialEq)]
struct TextureShape {
    width: u32,
    height: u32,
    mips: u32,
}

/// Whether an image carries a mipmap chain. Mips cost a CPU downsample per level
/// on every upload, so they are worth it only for an image that is drawn
/// *minified* (a 64px icon in a 16px slot). An image drawn at its native size -
/// and especially one refilled every frame, like a color picker's gradient - is
/// better off without them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mips {
    /// A full chain down to 1x1: for images that may be drawn smaller.
    Chain,
    /// Level 0 only: for images drawn at their native size.
    None,
}

impl Mips {
    /// How many levels this policy gives an image of `width` x `height`.
    fn levels(self, width: u32, height: u32) -> u32 {
        match self {
            Mips::Chain => 1 + width.max(height).max(1).ilog2(),
            Mips::None => 1,
        }
    }
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
            texture: None,
            shape: None,
        })
    }

    /// Register a registry-owned image from raw RGBA8 bytes (`width * height *
    /// 4`, row-major) - e.g. a rasterized icon. The texture is created, filled,
    /// and kept alive here, so the caller need not manage it.
    // Mirrors make_rgba's GPU handles plus the pixel source; a params struct
    // would not read more clearly.
    #[allow(clippy::too_many_arguments)]
    pub fn register_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        rgba: &[u8],
        width: u32,
        height: u32,
        mips: Mips,
    ) -> ImageHandle {
        let (texture, bind_group, shape) =
            self.make_rgba(device, queue, layout, rgba, width, height, mips);
        self.images.insert(RegisteredImage {
            bind_group,
            texture: Some(texture),
            shape: Some(shape),
        })
    }

    /// Replace a registry-owned image's pixels (e.g. the color picker's SV
    /// square as the hue changes), keeping its handle valid. Returns `false`
    /// on an unknown handle.
    ///
    /// A refill at the same size writes straight into the existing texture -
    /// no texture, view, or bind group is created, and the image keeps whatever
    /// mip policy it was registered with. That matters because this is the
    /// per-frame path for a live gradient: rebuilding the texture instead would
    /// churn a GPU allocation (and a full CPU mip chain) on every mouse move.
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
        // In-place refill: same size, and the registry owns the texture.
        if let Some(image) = self.images.get(handle)
            && let (Some(texture), Some(shape)) = (&image.texture, image.shape)
            && shape.width == width
            && shape.height == height
        {
            write_levels(queue, texture, rgba, width, height, shape.mips);
            return true;
        }
        // The size changed (or the caller owns the texture): build a new one,
        // keeping the previous mip policy so a resize does not silently drop
        // (or add) mips.
        let mips = match self.images.get(handle).and_then(|i| i.shape) {
            Some(s) if s.mips == 1 => Mips::None,
            _ => Mips::Chain,
        };
        let (texture, bind_group, shape) =
            self.make_rgba(device, queue, layout, rgba, width, height, mips);
        match self.images.get_mut(handle) {
            Some(image) => {
                image.bind_group = bind_group;
                image.texture = Some(texture);
                image.shape = Some(shape);
                true
            }
            None => false,
        }
    }

    /// Create a texture from RGBA8 bytes and its bind group (shared by
    /// register/update).
    #[allow(clippy::too_many_arguments)]
    fn make_rgba(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        rgba: &[u8],
        width: u32,
        height: u32,
        mips: Mips,
    ) -> (wgpu::Texture, wgpu::BindGroup, TextureShape) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let mip_level_count = mips.levels(width, height);
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
        write_levels(queue, &texture, rgba, width, height, mip_level_count);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.build_bind_group(device, layout, &view);
        (
            texture,
            bind_group,
            TextureShape {
                width,
                height,
                mips: mip_level_count,
            },
        )
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

/// Upload `rgba` as mip level 0 of `texture`, then box-downsample each further
/// level on the CPU and upload it. (Icon art is white with an alpha coverage
/// mask, so averaging is correct; for photos it is the usual good-enough sRGB
/// box filter.) With `mip_level_count` of 1 this is a single `write_texture`.
fn write_levels(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    rgba: &[u8],
    width: u32,
    height: u32,
    mip_level_count: u32,
) {
    // Borrow level 0 (the common single-level case copies nothing); later levels
    // own their downsampled buffer.
    let mut level: std::borrow::Cow<'_, [u8]> = std::borrow::Cow::Borrowed(rgba);
    let (mut lw, mut lh) = (width, height);
    for mip in 0..mip_level_count {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
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
            let (next, nw, nh) = downsample_2x2(&level, lw, lh);
            level = std::borrow::Cow::Owned(next);
            (lw, lh) = (nw, nh);
        }
    }
}

/// The 256 sRGB byte values decoded to linear light, computed once.
static SRGB_TO_LINEAR: std::sync::LazyLock<[f32; 256]> =
    std::sync::LazyLock::new(|| std::array::from_fn(srgb_to_linear));

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
    // The 256 possible sRGB byte values decoded to linear. Built once for the
    // whole process, not per call - this runs once per mip level per upload.
    let to_linear = &*SRGB_TO_LINEAR;
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
