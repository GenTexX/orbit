//! aurora-wgpu glyph atlas: rasterizes glyphs (via cosmic-text's swash cache) and packs them into one texture.

use std::collections::HashMap;

use cosmic_text::{CacheKey, FontSystem, SwashCache, SwashContent};

const ATLAS_SIZE: u32 = 1024;
/// One texel of padding between packed glyphs so linear sampling of one glyph
/// never bleeds into its neighbor.
const PAD: u32 = 1;
/// Side of the solid-white block reserved at the atlas origin, which filled
/// rectangles sample so they can share the glyph pipeline.
const WHITE: u32 = 4;

/// Where a rasterized glyph lives in the atlas, plus its bitmap bearing.
#[derive(Clone, Copy)]
pub(crate) struct GlyphEntry {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    /// Left bearing: x offset from the pen to the bitmap's left edge.
    pub left: i32,
    /// Top bearing: y offset from the baseline up to the bitmap's top edge.
    pub top: i32,
    pub width: f32,
    pub height: f32,
}

/// A single-channel (coverage) texture holding rasterized glyphs and one white
/// block, filled by a simple shelf packer.
pub(crate) struct Atlas {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    swash: SwashCache,
    glyphs: HashMap<CacheKey, Option<GlyphEntry>>,
    cursor_x: u32,
    cursor_y: u32,
    shelf_height: u32,
    /// UV of the center of the reserved white block, for solid fills.
    pub white_uv: [f32; 2],
}

impl Atlas {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // A solid-white block at the origin for filled rects to sample.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8; (WHITE * WHITE) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WHITE),
                rows_per_image: Some(WHITE),
            },
            wgpu::Extent3d {
                width: WHITE,
                height: WHITE,
                depth_or_array_layers: 1,
            },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            swash: SwashCache::new(),
            glyphs: HashMap::new(),
            // Start packing on a fresh shelf below the white block.
            cursor_x: 0,
            cursor_y: WHITE + PAD,
            shelf_height: 0,
            white_uv: [
                (WHITE as f32 / 2.0) / ATLAS_SIZE as f32,
                (WHITE as f32 / 2.0) / ATLAS_SIZE as f32,
            ],
        }
    }

    /// Ensure `cache_key`'s glyph is in the atlas, rasterizing and packing it on
    /// first sight. Returns `None` for glyphs with no bitmap (e.g. spaces) or
    /// unsupported color glyphs.
    pub fn ensure(
        &mut self,
        queue: &wgpu::Queue,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
    ) -> Option<GlyphEntry> {
        if let Some(entry) = self.glyphs.get(&cache_key) {
            return *entry;
        }

        // Copy the rasterized bitmap out so the swash-cache borrow ends before we
        // mutate the packer.
        let extracted = match self.swash.get_image(font_system, cache_key) {
            Some(image)
                if image.placement.width > 0
                    && image.placement.height > 0
                    && matches!(image.content, SwashContent::Mask) =>
            {
                Some((image.placement, image.data.clone()))
            }
            _ => None,
        };

        let entry = extracted.map(|(placement, data)| {
            let (w, h) = (placement.width, placement.height);
            let (x, y) = self.pack(w, h);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            let s = ATLAS_SIZE as f32;
            GlyphEntry {
                uv_min: [x as f32 / s, y as f32 / s],
                uv_max: [(x + w) as f32 / s, (y + h) as f32 / s],
                left: placement.left,
                top: placement.top,
                width: w as f32,
                height: h as f32,
            }
        });

        self.glyphs.insert(cache_key, entry);
        entry
    }

    /// Shelf packer: place a `w` x `h` glyph and advance the cursor. Assumes the
    /// atlas is large enough (1024x1024 far exceeds an inspector's glyph set);
    /// growing or evicting is a later concern.
    fn pack(&mut self, w: u32, h: u32) -> (u32, u32) {
        if self.cursor_x + w + PAD > ATLAS_SIZE {
            self.cursor_x = 0;
            self.cursor_y += self.shelf_height + PAD;
            self.shelf_height = 0;
        }
        debug_assert!(self.cursor_y + h <= ATLAS_SIZE, "glyph atlas is full");
        let (x, y) = (self.cursor_x, self.cursor_y);
        self.cursor_x += w + PAD;
        self.shelf_height = self.shelf_height.max(h);
        (x, y)
    }
}
