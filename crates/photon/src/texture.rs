//! photon texture: an RGBA image living on the GPU, sampled by the sprite pipeline.

/// A 2D texture on the GPU. Created from raw RGBA8 bytes; image *decoding*
/// (PNG, etc.) stays outside the renderer, keeping this crate free of asset
/// concerns.
pub struct Texture {
    pub(crate) view: wgpu::TextureView,
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
}

impl Texture {
    /// Upload `rgba` (tightly packed, `width * height * 4` bytes, row-major) as
    /// an sRGB texture. Panics if the byte count doesn't match the dimensions.
    pub(crate) fn from_rgba(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Self {
        assert_eq!(
            rgba.len(),
            (width * height * 4) as usize,
            "RGBA byte count must equal width * height * 4"
        );

        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Sprite art is authored in sRGB; sampling returns linear values.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            view,
            width,
            height,
        }
    }
}
