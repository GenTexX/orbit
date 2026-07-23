//! atlas demo project: a small on-disk Project (ADR 0009) the editor opens at
//! startup, bootstrapped once if it does not exist yet.

use std::path::Path;

use anyhow::{Context, Result};
use glam::Vec2;
use helios::{Component, Node, Project, Scene, SpriteComponent, Transform};

/// The bundled sample sprite, written into the demo project's `assets/` the
/// first time it is created - so the file explorer has a real PNG to list and
/// the viewport a real texture to show, not a flat tint (M3 loads PNGs
/// directly; a full import pipeline is later work).
const SPRITE_PNG: &[u8] = include_bytes!("../assets/sprite.png");

/// Open the demo project at `dir`, creating it on first run.
pub fn open_or_create(dir: &Path) -> Result<Project> {
    if dir.join("orbit.toml").exists() {
        return Project::load(dir).with_context(|| format!("load project at {}", dir.display()));
    }

    std::fs::create_dir_all(dir.join("assets")).context("create assets directory")?;
    std::fs::write(dir.join("assets/sprite.png"), SPRITE_PNG).context("write sprite.png")?;

    let mut scene = Scene::new("Root");
    let root = scene.root();
    let mut sprite = Node::new("Sprite");
    sprite.transform = Transform::from_translation(Vec2::new(60.0, 60.0));
    sprite.components.push(Component::Sprite(SpriteComponent {
        texture: "assets/sprite.png".to_string(),
        tint: [1.0, 1.0, 1.0, 1.0],
        size: Vec2::new(160.0, 160.0),
    }));
    scene.add_child(root, sprite);

    let project = Project::new("Demo", scene);
    project
        .save(dir)
        .with_context(|| format!("save new project to {}", dir.display()))?;
    Ok(project)
}
