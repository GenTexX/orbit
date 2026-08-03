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

/// Write `bytes` to `path` without ever leaving it half-written: a temp sibling
/// is written and flushed first, then renamed over the target.
///
/// `std::fs::write` truncates the file before it writes, so a crash or a full
/// disk mid-save leaves an empty or partial file and the original is gone. The
/// temp name starts with a dot so a file explorer that hides dotfiles does not
/// flicker it into the list.
///
/// Here rather than in the editor because the thing most worth not losing - the
/// scene - is written from here, and M6's package writer will want it too.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
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
    ///
    /// Both go through [`write_atomic`], so an interrupted save leaves the
    /// previous good file rather than a truncated one. The scene is the least
    /// replaceable thing in a project and it used to get the weakest treatment.
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
        write_atomic(&manifest_path, manifest_text.as_bytes())
            .map_err(|e| HeliosError::io("writing", &manifest_path, e))?;

        let scene_path = dir.join(MAIN_SCENE);
        if let Some(parent) = scene_path.parent() {
            fs::create_dir_all(parent).map_err(|e| HeliosError::io("creating", parent, e))?;
        }
        write_atomic(&scene_path, self.scene.to_ron()?.as_bytes())
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

    #[test]
    fn a_failed_write_leaves_the_previous_file_whole() {
        // The point of writing through a temp sibling: `fs::write` truncates
        // first, so an interrupted save used to leave an empty scene and take
        // the only good copy with it. Simulated by making the rename fail - the
        // target is a directory, which cannot be replaced by a file.
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("scene.ron");
        fs::write(&good, "the previous good scene").unwrap();
        assert!(write_atomic(&good, b"a new scene").is_ok());
        assert_eq!(fs::read_to_string(&good).unwrap(), "a new scene");

        let blocked = dir.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        assert!(
            write_atomic(&blocked, b"cannot land").is_err(),
            "renaming a file over a directory must fail"
        );
        assert!(blocked.is_dir(), "and must leave what was there alone");
        // No litter either: the temp sibling is cleaned up when the rename fails.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }
}
