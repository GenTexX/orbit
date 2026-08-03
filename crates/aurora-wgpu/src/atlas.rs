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

/// Where the next glyph goes: a row of glyphs at a time, left to right, then
/// down by the tallest one on the row.
///
/// Split out from [`Atlas`] because it is arithmetic and nothing else - the
/// rest of that type needs a GPU to exist, which is why the one part of it that
/// can be wrong in an interesting way had no test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shelf {
    x: u32,
    y: u32,
    height: u32,
}

/// A shelf starts *below* the reserved white block, never at the origin.
///
/// Written as `Default` rather than a named constructor so there is no
/// zero-valued shelf to reach for by accident. The origin belongs to the white
/// block that every solid fill samples: a shelf starting at (0, 0) packs the
/// first glyph over it, and then every filled rect in the application samples
/// whatever that glyph has at its centre - which is often nothing, and the
/// fills come out transparent. That is a whole-application symptom from one
/// wrong initializer, and it happened.
impl Default for Shelf {
    fn default() -> Self {
        Self {
            x: 0,
            y: WHITE + PAD,
            height: 0,
        }
    }
}

impl Shelf {
    /// Place a `w` x `h` glyph, or `None` when there is no room left.
    ///
    /// `None` rather than a `debug_assert`: past the last shelf the old version
    /// handed back an origin outside the texture, `write_texture` raised a wgpu
    /// validation error, and wgpu's default handler turns that into an abort.
    /// A release build therefore killed the editor when the atlas filled.
    fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.x + w + PAD > ATLAS_SIZE {
            self.x = 0;
            self.y += self.height + PAD;
            self.height = 0;
        }
        if self.y + h > ATLAS_SIZE {
            return None;
        }
        let at = (self.x, self.y);
        self.x += w + PAD;
        self.height = self.height.max(h);
        Some(at)
    }
}

/// A single-channel (coverage) texture holding rasterized glyphs and one white
/// block, filled by a simple shelf packer.
pub(crate) struct Atlas {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    swash: SwashCache,
    glyphs: HashMap<CacheKey, Option<GlyphEntry>>,
    shelf: Shelf,
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
            shelf: Shelf::default(),
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

        let entry = extracted.and_then(|(placement, data)| {
            let (w, h) = (placement.width, placement.height);
            // One retry after a reset. A glyph too big for an empty atlas is
            // the only way to fail twice, and that one is genuinely undrawable.
            let (x, y) = match self.pack(w, h) {
                Some(at) => at,
                None => {
                    self.reset();
                    self.pack(w, h)?
                }
            };
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
            Some(GlyphEntry {
                uv_min: [x as f32 / s, y as f32 / s],
                uv_max: [(x + w) as f32 / s, (y + h) as f32 / s],
                left: placement.left,
                top: placement.top,
                width: w as f32,
                height: h as f32,
            })
        });

        self.glyphs.insert(cache_key, entry);
        entry
    }

    /// Shelf packer: place a `w` x `h` glyph and advance the cursor, or `None`
    /// when the atlas has no room left.
    ///
    /// `None` rather than a `debug_assert`: past the last shelf the old version
    /// handed back an origin outside the texture, `write_texture` raised a wgpu
    /// validation error, and wgpu's default handler turns that into an abort.
    /// A release build therefore *killed the editor* when the atlas filled -
    /// which is reachable, since 1024x1024 is a few thousand glyphs and a large
    /// font size, several faces, or a script full of unusual characters all
    /// spend it faster than an inspector does.
    fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        self.shelf.place(w, h)
    }

    /// Forget every glyph and start packing from the top again.
    ///
    /// The blunt survivable answer to a full atlas: the texture keeps whatever
    /// bytes it had, but nothing refers to them any more, and the glyphs still
    /// on screen are re-rasterized into the fresh space over the next frames. A
    /// hitch instead of a crash. An LRU pass or a second page would be better;
    /// this is what makes the failure recoverable at all. Silent because this
    /// crate takes no logging dependency - the visible sign is the one hitched
    /// frame.
    fn reset(&mut self) {
        self.glyphs.clear();
        // Below the white block again, not at the origin: clearing the shelf
        // does not clear the texture, and the block is still where it was.
        self.shelf = Shelf::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_shelf_says_so_instead_of_pointing_outside_the_texture() {
        // The failure this replaces: past the last row the packer returned an
        // origin beyond the texture, which wgpu rejects and its default error
        // handler turns into a process abort - so a full glyph atlas took the
        // editor with it.
        let mut shelf = Shelf::default();
        let top = WHITE + PAD;
        assert_eq!(
            shelf.place(10, 10),
            Some((0, top)),
            "clear of the white block"
        );
        assert_eq!(shelf.place(10, 10), Some((10 + PAD, top)), "along the row");

        // Fill it. Tall rows so this does not take thousands of iterations.
        let tall = 256;
        let mut placed = 2;
        while shelf.place(ATLAS_SIZE - PAD - 1, tall).is_some() {
            placed += 1;
            assert!(placed < 10_000, "it should run out, not go forever");
        }
        // Everything it did hand out was inside the texture.
        assert!(shelf.y + tall > ATLAS_SIZE, "it stopped at the bottom edge");

        // And starting again is what makes a full atlas a hitch rather than a
        // crash - starting again *below the white block*, because clearing the
        // shelf does not clear the texture and the block is still there.
        let mut fresh = Shelf::default();
        assert_eq!(fresh.place(10, 10), Some((0, WHITE + PAD)));
    }

    #[test]
    fn nothing_is_ever_packed_over_the_white_block() {
        // Every solid fill in the application samples one point - the centre of
        // the 4x4 white block at the atlas origin - so a glyph packed on top of
        // it makes every filled rect sample that glyph instead. Depending on
        // what landed there, fills come out faint or invisible, which is what
        // "the popup is transparent" looked like. Worth a test rather than a
        // constant, because the shelf is now built in two places.
        let mut shelf = Shelf::default();
        let mut placed = 0;
        while let Some((x, y)) = shelf.place(7, 9) {
            assert!(
                y >= WHITE + PAD || x >= WHITE + PAD,
                "packed at ({x}, {y}), which is inside the white block"
            );
            placed += 1;
            assert!(placed < 100_000, "it should fill up, not run forever");
        }
        assert!(placed > 100, "it packed a realistic number of glyphs");
    }

    #[test]
    fn a_glyph_taller_than_the_whole_atlas_is_refused_rather_than_placed() {
        let mut shelf = Shelf::default();
        assert_eq!(shelf.place(8, ATLAS_SIZE + 1), None);
    }
}
