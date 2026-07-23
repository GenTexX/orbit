//! atlas texture cache: photon textures keyed by project-relative path, loaded
//! lazily from disk. Without it every sprite drew with one shared texture
//! regardless of the path it named; with it each sprite shows its own art, and
//! the renderer batches consecutive same-texture sprites (M3 iteration 3).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use helios::SpriteDraw;
use photon::{Renderer, Sprite, Texture};

/// Coalesce sprites (already in paint order) into runs of consecutive
/// same-texture sprites - the batches a renderer draws in order, so painter's
/// order is preserved while adjacent same-texture sprites share a draw.
pub fn coalesce_runs(draws: &[SpriteDraw]) -> Vec<(String, Vec<Sprite>)> {
    let mut runs: Vec<(String, Vec<Sprite>)> = Vec::new();
    for draw in draws {
        match runs.last_mut() {
            Some(last) if last.0 == draw.texture => last.1.push(draw.sprite),
            _ => runs.push((draw.texture.clone(), vec![draw.sprite])),
        }
    }
    runs
}

/// A path -> [`Texture`] cache. A missing or unreadable path falls back to a
/// magenta placeholder (the classic "texture not found" tell) and is
/// remembered as failed so it is not retried every frame.
pub struct TextureCache {
    textures: HashMap<String, Texture>,
    failed: HashSet<String>,
    fallback: Texture,
}

impl TextureCache {
    /// Create an empty cache with a 1x1 magenta fallback texture.
    pub fn new(engine: &Renderer) -> Self {
        Self {
            textures: HashMap::new(),
            failed: HashSet::new(),
            fallback: engine.create_texture(&[255, 0, 255, 255], 1, 1),
        }
    }

    /// Ensure `path` (project-relative) is loaded, decoding it from disk on the
    /// first request. Idempotent and cheap on a hit; a load failure is logged
    /// once and the path is marked failed so it uses the fallback thereafter.
    pub fn ensure(&mut self, engine: &Renderer, project_dir: &Path, path: &str) {
        if self.textures.contains_key(path) || self.failed.contains(path) {
            return;
        }
        match load_texture(engine, &project_dir.join(path)) {
            Ok(texture) => {
                self.textures.insert(path.to_string(), texture);
            }
            Err(err) => {
                tracing::warn!("texture '{path}' failed to load ({err:#}); using fallback");
                self.failed.insert(path.to_string());
            }
        }
    }

    /// The texture for `path`, or the magenta fallback if it is not loaded.
    /// Call [`ensure`](Self::ensure) first so a real texture is available.
    pub fn get(&self, path: &str) -> &Texture {
        self.textures.get(path).unwrap_or(&self.fallback)
    }
}

/// Decode a PNG (or other image) from disk into a photon texture.
fn load_texture(engine: &Renderer, path: &Path) -> Result<Texture> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let rgba = image::load_from_memory(&bytes)
        .with_context(|| format!("decode {}", path.display()))?
        .to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(engine.create_texture(&rgba.into_raw(), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use helios::NodeId;
    use slotmap::KeyData;

    fn draw(texture: &str) -> SpriteDraw {
        // A throwaway NodeId: coalescing never inspects it.
        SpriteDraw {
            node: NodeId::from(KeyData::from_ffi(1)),
            texture: texture.to_string(),
            sprite: Sprite::new(Vec2::ZERO, Vec2::ONE),
        }
    }

    #[test]
    fn coalesce_groups_only_consecutive_same_texture_sprites() {
        // a a b a -> three runs (the trailing `a` does NOT merge with the
        // first run, which would reorder it past `b` and break paint order).
        let draws = [draw("a"), draw("a"), draw("b"), draw("a")];
        let runs = coalesce_runs(&draws);
        let shape: Vec<(String, usize)> = runs.iter().map(|(t, s)| (t.clone(), s.len())).collect();
        assert_eq!(
            shape,
            vec![
                ("a".to_string(), 2),
                ("b".to_string(), 1),
                ("a".to_string(), 1),
            ]
        );
    }

    #[test]
    fn coalesce_of_nothing_is_no_runs() {
        assert!(coalesce_runs(&[]).is_empty());
    }
}
