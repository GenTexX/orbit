//! atlas image thumbnails: decode a project image once, downscaled onto a
//! square transparent canvas, and register it with the GUI renderer so the file
//! explorer can preview it (via the same `register_image_rgba` path the toolbar
//! icons use). Cached by absolute path; each image is decoded at most once, and
//! a failed decode is remembered as `None` so it is not retried every frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aurora::ImageHandle;
use aurora_wgpu::Renderer as AuroraRenderer;

/// The side length (in pixels) of a thumbnail's square canvas. The image is fit
/// inside preserving aspect; the padding is transparent, so the explorer's
/// square image widget shows it undistorted (the widget stretches to its rect).
const THUMB: u32 = 64;

/// A cache of project-image thumbnails, keyed by absolute path.
#[derive(Default)]
pub struct Thumbnails {
    /// `None` marks a path that failed to decode (undecodable format, unreadable
    /// file): kept so it is not retried, distinct from an absent (not-yet-tried)
    /// entry.
    cache: HashMap<PathBuf, Option<ImageHandle>>,
}

impl Thumbnails {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensure a thumbnail for `path` is decoded and registered, returning its
    /// handle (or `None` if it could not be decoded). Decodes at most once per
    /// path; subsequent calls are a cheap map lookup.
    pub fn ensure(&mut self, gui: &mut AuroraRenderer, path: &Path) -> Option<ImageHandle> {
        if let Some(&cached) = self.cache.get(path) {
            return cached;
        }
        let handle = decode_square(path).map(|rgba| gui.register_image_rgba(&rgba, THUMB, THUMB));
        self.cache.insert(path.to_path_buf(), handle);
        handle
    }

    /// The handle for an already-ensured thumbnail, or `None` if it was never
    /// ensured or failed to decode.
    pub fn get(&self, path: &Path) -> Option<ImageHandle> {
        self.cache.get(path).copied().flatten()
    }

    /// Whether `path` has already been attempted (decoded OR failed) - so a
    /// caller can skip both without re-attempting, unlike [`get`](Self::get)
    /// which cannot tell "failed" from "never tried".
    pub fn contains(&self, path: &Path) -> bool {
        self.cache.contains_key(path)
    }

    /// Re-decode every cached thumbnail in place, reusing each path's existing
    /// GPU texture (`update_image_rgba`) rather than registering a new one.
    /// Called on a project reload or a Refresh, where the files at those paths
    /// may have changed. Reusing the texture keeps the renderer's image count
    /// bounded by the set of distinct paths seen, instead of leaking a fresh
    /// texture on every refresh (the registry has no deregister path).
    pub fn invalidate(&mut self, gui: &mut AuroraRenderer) {
        for (path, slot) in self.cache.iter_mut() {
            match decode_square(path) {
                Some(rgba) => match *slot {
                    Some(handle) => {
                        gui.update_image_rgba(handle, &rgba, THUMB, THUMB);
                    }
                    None => *slot = Some(gui.register_image_rgba(&rgba, THUMB, THUMB)),
                },
                // No longer decodable (deleted or corrupt): fall back to the type
                // icon. Its old texture lingers, but no new one is allocated.
                None => *slot = None,
            }
        }
    }
}

/// Decode `path`, scale it to fit within `THUMB` x `THUMB` preserving aspect,
/// and center it on a `THUMB` x `THUMB` transparent RGBA canvas. `None` if the
/// bytes cannot be read or decoded.
fn decode_square(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    // `thumbnail` scales down preserving aspect, fitting inside the box.
    let scaled = image::load_from_memory(&bytes)
        .ok()?
        .thumbnail(THUMB, THUMB);
    let rgba = scaled.to_rgba8();
    let (w, h) = rgba.dimensions();
    let side = THUMB as usize;
    let mut canvas = vec![0u8; side * side * 4];
    let ox = (side - w as usize) / 2;
    let oy = (side - h as usize) / 2;
    let src = rgba.as_raw();
    for row in 0..h as usize {
        let s = row * w as usize * 4;
        let d = ((row + oy) * side + ox) * 4;
        canvas[d..d + w as usize * 4].copy_from_slice(&src[s..s + w as usize * 4]);
    }
    Some(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a solid opaque WxH PNG to a temp file and return its path.
    fn write_png(dir: &Path, w: u32, h: u32) -> PathBuf {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]));
        let path = dir.join(format!("{w}x{h}.png"));
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn decode_square_letterboxes_onto_a_fixed_transparent_canvas() {
        let dir = tempfile::tempdir().unwrap();
        // A wide 40x20 image fits to 64x32 and centers, so the top and bottom
        // rows stay transparent and the middle is opaque - and either way the
        // canvas is exactly THUMB x THUMB.
        let wide = decode_square(&write_png(dir.path(), 40, 20)).unwrap();
        assert_eq!(wide.len(), (THUMB * THUMB * 4) as usize);
        let px = |x: u32, y: u32| wide[((y * THUMB + x) * 4 + 3) as usize];
        assert_eq!(px(THUMB / 2, 0), 0, "top row is transparent padding");
        assert_ne!(px(THUMB / 2, THUMB / 2), 0, "center is opaque image");

        // A square image fills the whole canvas.
        let square = decode_square(&write_png(dir.path(), 100, 100)).unwrap();
        assert_eq!(square.len(), (THUMB * THUMB * 4) as usize);

        // Extreme aspect ratios and a 1x1 image must not panic the letterbox
        // index math (the bounds safety hinges on `image`'s fit-box rounding
        // never exceeding THUMB in either dimension).
        for (w, h) in [(1, 1), (1000, 1), (1, 1000), (2, 200), (200, 2)] {
            let out = decode_square(&write_png(dir.path(), w, h)).unwrap();
            assert_eq!(out.len(), (THUMB * THUMB * 4) as usize, "{w}x{h}");
        }
    }

    #[test]
    fn decode_square_returns_none_for_undecodable_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-an-image.png");
        std::fs::write(&path, b"totally not a png").unwrap();
        assert!(decode_square(&path).is_none());
        assert!(decode_square(&dir.path().join("missing.png")).is_none());
    }
}
