//! helios project: the on-disk authoring artifact - a directory with an
//! `orbit.toml` manifest and a scene file (ADR 0009), not a single blob.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::HeliosError;
use crate::scene::Scene;

/// The manifest file name at the root of a project directory.
const MANIFEST: &str = "orbit.toml";
/// Where the scene is written, relative to the project directory.
const MAIN_SCENE: &str = "scenes/main.ron";

/// The project manifest (`orbit.toml`): what the editor reads to open a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// The project's display name.
    pub name: String,
    /// The main scene file, relative to the project directory.
    pub main_scene: String,
}

/// An open project: its name and its loaded scene. A project is a directory of
/// text files (ADR 0009); M3 holds a single scene.
#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub scene: Scene,
}

impl Project {
    /// A new project named `name` wrapping `scene`.
    pub fn new(name: impl Into<String>, scene: Scene) -> Self {
        Self {
            name: name.into(),
            scene,
        }
    }

    /// Write the project to `dir`: the manifest and the scene file, creating the
    /// directory (and the scene's parent) as needed.
    pub fn save(&self, dir: impl AsRef<Path>) -> Result<(), HeliosError> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir).map_err(|e| HeliosError::io("creating", dir, e))?;

        let manifest = Manifest {
            name: self.name.clone(),
            main_scene: MAIN_SCENE.to_string(),
        };
        let manifest_text =
            toml::to_string_pretty(&manifest).map_err(|e| HeliosError::Manifest(e.to_string()))?;
        let manifest_path = dir.join(MANIFEST);
        fs::write(&manifest_path, manifest_text)
            .map_err(|e| HeliosError::io("writing", &manifest_path, e))?;

        let scene_path = dir.join(MAIN_SCENE);
        if let Some(parent) = scene_path.parent() {
            fs::create_dir_all(parent).map_err(|e| HeliosError::io("creating", parent, e))?;
        }
        fs::write(&scene_path, self.scene.to_ron()?)
            .map_err(|e| HeliosError::io("writing", &scene_path, e))?;
        Ok(())
    }

    /// Load a project from `dir` by reading its manifest and scene file.
    pub fn load(dir: impl AsRef<Path>) -> Result<Project, HeliosError> {
        let dir = dir.as_ref();
        let manifest_path = dir.join(MANIFEST);
        let manifest_text = fs::read_to_string(&manifest_path)
            .map_err(|e| HeliosError::io("reading", &manifest_path, e))?;
        let manifest: Manifest =
            toml::from_str(&manifest_text).map_err(|e| HeliosError::Manifest(e.to_string()))?;

        let scene_path = dir.join(&manifest.main_scene);
        let scene_text = fs::read_to_string(&scene_path)
            .map_err(|e| HeliosError::io("reading", &scene_path, e))?;
        let scene = Scene::from_ron(&scene_text)?;
        Ok(Project {
            name: manifest.name,
            scene,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{Component, SpriteComponent};
    use crate::scene::Node;

    #[test]
    fn a_project_round_trips_through_a_directory() {
        let mut scene = Scene::new("root");
        let root = scene.root();
        let mut player = Node::new("player");
        player
            .components
            .push(Component::Sprite(SpriteComponent::default()));
        scene.add_child(root, player);

        let project = Project::new("demo", scene);
        let dir = tempfile::tempdir().unwrap();
        project.save(dir.path()).unwrap();

        // The manifest and scene file land where the manifest says.
        assert!(dir.path().join("orbit.toml").exists());
        assert!(dir.path().join("scenes/main.ron").exists());

        let loaded = Project::load(dir.path()).unwrap();
        assert_eq!(loaded.name, "demo");
        assert_eq!(
            loaded.scene.to_ron().unwrap(),
            project.scene.to_ron().unwrap()
        );
    }
}
