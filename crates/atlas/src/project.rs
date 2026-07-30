//! atlas demo project: a small on-disk Project (ADR 0009) the editor opens at
//! startup, bootstrapped once if it does not exist yet.

use std::path::Path;

use anyhow::{Context, Result};
use glam::Vec2;
use helios::{Component, Node, Project, Scene, ScriptComponent, SpriteComponent, Transform};

/// The bundled sample sprite, written into the demo project's `assets/` the
/// first time it is created - so the file explorer has a real PNG to list and
/// the viewport a real texture to show, not a flat tint (M3 loads PNGs
/// directly; a full import pipeline is later work).
const SPRITE_PNG: &[u8] = include_bytes!("../assets/sprite.png");

/// Two Comet scripts, written into `scripts/` on first run. The first is the
/// language's whole surface in thirty lines, attached to the demo sprite so the
/// Script component is visible in the inspector without anyone having to build
/// one; the second is wrong on purpose, so the first thing a person sees in the
/// Code pane can be what a diagnostic looks like.
const BOUNCE_CMT: &str = include_str!("../demo_project/scripts/bounce.cmt");
const SQUIGGLES_CMT: &str = include_str!("../demo_project/scripts/squiggles.cmt");

/// Open the demo project at `dir`, creating it on first run.
pub fn open_or_create(dir: &Path) -> Result<Project> {
    if dir.join("orbit.toml").exists() {
        return Project::load(dir).with_context(|| format!("load project at {}", dir.display()));
    }

    std::fs::create_dir_all(dir.join("assets")).context("create assets directory")?;
    std::fs::write(dir.join("assets/sprite.png"), SPRITE_PNG).context("write sprite.png")?;
    std::fs::create_dir_all(dir.join("scripts")).context("create scripts directory")?;
    std::fs::write(dir.join("scripts/bounce.cmt"), BOUNCE_CMT).context("write bounce.cmt")?;
    std::fs::write(dir.join("scripts/squiggles.cmt"), SQUIGGLES_CMT)
        .context("write squiggles.cmt")?;

    let mut scene = Scene::new("Root");
    let root = scene.root();
    let mut sprite = Node::new("Sprite");
    // The node origin is the sprite's center (ADR 0019); place it so the
    // 160x160 quad sits fully inside the viewport at the default camera.
    sprite.transform = Transform::from_translation(Vec2::new(200.0, 160.0));
    sprite.components.push(Component::Sprite(SpriteComponent {
        texture: "assets/sprite.png".to_string(),
        tint: [1.0, 1.0, 1.0, 1.0],
        size: Vec2::new(160.0, 160.0),
    }));
    sprite.components.push(Component::Script(ScriptComponent {
        source: "scripts/bounce.cmt".to_string(),
    }));
    scene.add_child(root, sprite);

    let project = Project::new("Demo", scene);
    project
        .save(dir)
        .with_context(|| format!("save new project to {}", dir.display()))?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_project_ships_a_script_that_compiles_and_one_that_does_not() {
        // This path only runs on a first launch, so nothing else would ever
        // exercise it - and it writes the first Comet anyone sees.
        let dir = tempfile::tempdir().unwrap();
        let project = open_or_create(dir.path()).expect("scaffold a project");

        let bounce = std::fs::read_to_string(dir.path().join("scripts/bounce.cmt")).unwrap();
        assert!(
            comet::compile(&bounce).is_ok(),
            "the demo script must compile: {:?}",
            comet::compile(&bounce).err()
        );

        // The broken one is broken on purpose, and its errors are the three it
        // has comments about - a squiggle each, not one that stops the rest.
        let squiggles = std::fs::read_to_string(dir.path().join("scripts/squiggles.cmt")).unwrap();
        let diagnostics = comet::service::diagnostics(&squiggles);
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert!(diagnostics.iter().all(|d| d.span.end > d.span.start));

        // And the demo sprite already has the script attached, so the Script
        // component is in the inspector without anyone building one.
        let sprite = project.scene.children(project.scene.root())[0];
        let script = project
            .scene
            .node(sprite)
            .components
            .iter()
            .find_map(|c| match c {
                Component::Script(s) => Some(s),
                _ => None,
            });
        assert_eq!(
            script.map(|s| s.source.as_str()),
            Some("scripts/bounce.cmt")
        );
    }

    #[test]
    fn opening_an_existing_project_does_not_re_scaffold_it() {
        let dir = tempfile::tempdir().unwrap();
        open_or_create(dir.path()).unwrap();
        std::fs::write(dir.path().join("scripts/bounce.cmt"), "// edited\n").unwrap();
        open_or_create(dir.path()).expect("load, not create");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("scripts/bounce.cmt")).unwrap(),
            "// edited\n",
            "a second launch must not overwrite the user's edits"
        );
    }
}
