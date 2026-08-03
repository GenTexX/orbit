//! Play mode: the editor's half of running a game.
//!
//! The loop itself is `voyager::Runtime` and lives in voyager on purpose (ADR
//! 0002). What is editor-specific, and therefore here, is that pressing Play in
//! an editor is a *rehearsal*: the authored scene is the document, the running
//! game is allowed to scribble on it, and Stop puts the document back. A shipped
//! game has nothing to put back, which is why voyager does not know about any of
//! this.

use std::path::PathBuf;

use helios::Scene;
use voyager::Runtime;

/// The editor's play session: a runtime, and the scene as it was before it ran.
pub struct Play {
    /// The project directory, kept because the runtime is not built until it is
    /// needed.
    root: PathBuf,
    /// Built on the first Play and kept afterwards.
    ///
    /// Lazy because a wasmtime `Engine` is expensive to stand up and it owns the
    /// epoch ticker thread that enforces the frame budget. An editing session
    /// that never presses Play should not pay for either, and most sessions
    /// spend most of their time not playing.
    runtime: Option<Runtime>,
    /// The authored scene, cloned when Play was pressed and put back on Stop.
    ///
    /// A clone rather than a RON round trip. Both would restore the values, but
    /// `Scene::from_ron` renumbers every `NodeId`, and the editor holds ids
    /// outside the scene - the selection, which nodes are collapsed in the tree,
    /// which inspector sections are open. Cloning keeps slotmap's keys, so all
    /// of that still points at the node it meant.
    ///
    /// `Some` exactly while a game is running.
    authored: Option<Scene>,
}

impl Play {
    /// A session for the project at `root`, not playing and holding no runtime
    /// yet.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Play {
            root: root.into(),
            runtime: None,
            authored: None,
        }
    }

    /// Whether a game is running.
    pub fn is_playing(&self) -> bool {
        self.runtime.as_ref().is_some_and(Runtime::is_playing)
    }

    /// Snapshot the scene and start every script in it.
    ///
    /// A runtime that will not start at all is a broken wasmtime rather than a
    /// broken project, so it is reported and nothing happens - the editor keeps
    /// working, which is the right outcome for a failure that has nothing to do
    /// with what the user was editing.
    pub fn start(&mut self, scene: &mut Scene) {
        if self.is_playing() {
            return;
        }
        if self.runtime.is_none() {
            match Runtime::new(self.root.clone()) {
                Ok(runtime) => self.runtime = Some(runtime),
                Err(err) => {
                    tracing::error!("the script runtime could not start: {err}");
                    return;
                }
            }
        }
        let Some(runtime) = self.runtime.as_mut() else {
            return;
        };
        // Taken before anything runs, because a script's `start` is allowed to
        // move the node it runs for and that is already part of the game.
        self.authored = Some(scene.clone());
        runtime.play(scene);
    }

    /// Stop the game and put the authored scene back.
    ///
    /// `on_destroy` runs first, against the scene the game left behind, because
    /// a script's last word is about the game it was in rather than about the
    /// document it is being replaced by.
    pub fn stop(&mut self, scene: &mut Scene) {
        if !self.is_playing() {
            return;
        }
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.stop(scene);
        }
        if let Some(authored) = self.authored.take() {
            *scene = authored;
        }
    }

    /// Run one frame, if a game is running.
    pub fn step(&mut self, scene: &mut Scene, dt: f32) {
        if !self.is_playing() {
            return;
        }
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.step(scene, dt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use helios::{Component, Node, NodeId, ScriptComponent};

    /// A project directory holding `move.cmt`, and a scene whose one child node
    /// runs it.
    fn project() -> (tempfile::TempDir, Scene, NodeId) {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(
            dir.path().join("move.cmt"),
            "func update(dt: f32) { transform.position.x += 10.0; }",
        )
        .expect("writing the script");

        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("player"));
        scene.node_mut(node).transform.translation.x = 3.0;
        scene
            .node_mut(node)
            .components
            .push(Component::Script(ScriptComponent {
                source: "move.cmt".to_string(),
                ..ScriptComponent::default()
            }));
        (dir, scene, node)
    }

    #[test]
    fn play_then_stop_leaves_the_scene_exactly_as_it_was() {
        // The property the whole mode rests on. If Stop does not restore, Play
        // is not a rehearsal, it is an edit nobody asked for - and the user
        // finds out when they save.
        let (dir, mut scene, node) = project();
        let before = scene.to_ron().expect("the scene serializes");

        let mut play = Play::new(dir.path());
        play.start(&mut scene);
        for _ in 0..5 {
            play.step(&mut scene, 0.016);
        }
        assert_eq!(
            scene.node(node).transform.translation.x,
            53.0,
            "the game really did move it"
        );

        play.stop(&mut scene);
        assert_eq!(
            scene.to_ron().expect("the scene serializes"),
            before,
            "and Stop put every byte back"
        );
    }

    #[test]
    fn the_node_ids_survive_so_the_editors_selection_does() {
        // Why the snapshot is a clone and not a RON round trip: the editor
        // holds NodeIds outside the scene - the selection, the collapsed rows -
        // and `Scene::from_ron` renumbers every one of them. A restore that
        // deselects what you had selected is a restore that lost something.
        let (dir, mut scene, node) = project();
        let mut play = Play::new(dir.path());

        play.start(&mut scene);
        play.step(&mut scene, 0.016);
        play.stop(&mut scene);

        assert_eq!(
            scene.node(node).name,
            "player",
            "the id the editor was holding still names the node it meant"
        );
    }

    #[test]
    fn the_demo_project_actually_plays() {
        // Not a fixture: the project the editor opens, with the scene and the
        // script it ships. It is the one test that would catch the source path
        // being resolved against the wrong directory, or the scene's stored
        // `@export` values not reaching the running script - both of which are
        // invisible in a fixture that builds its own paths.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("demo_project");
        let project = helios::Project::load(&dir).expect("the demo project loads");
        let mut scene = project.scene;
        let root = scene.root();
        let scripted =
            scene
                .children(root)
                .to_vec()
                .into_iter()
                .find(|&node| {
                    scene.node(node).components.iter().any(
                        |c| matches!(c, Component::Script(s) if s.source.ends_with("tunable.cmt")),
                    )
                })
                .expect("the demo scene runs tunable.cmt");
        let before = scene.node(scripted).transform.translation;

        let mut play = Play::new(&dir);
        play.start(&mut scene);
        assert!(play.is_playing(), "the demo project's script compiled");
        for _ in 0..10 {
            play.step(&mut scene, 0.016);
        }

        assert_ne!(
            scene.node(scripted).transform.translation,
            before,
            "ten frames of the shipped script moved the shipped node"
        );
        play.stop(&mut scene);
        assert_eq!(scene.node(scripted).transform.translation, before);
    }

    #[test]
    fn is_playing_tracks_start_and_stop() {
        let (dir, mut scene, _node) = project();
        let mut play = Play::new(dir.path());

        assert!(!play.is_playing());
        play.start(&mut scene);
        assert!(play.is_playing());
        play.stop(&mut scene);
        assert!(!play.is_playing());
    }

    #[test]
    fn stepping_while_stopped_does_nothing() {
        let (dir, mut scene, node) = project();
        let mut play = Play::new(dir.path());
        play.step(&mut scene, 0.016);
        assert_eq!(scene.node(node).transform.translation.x, 3.0);
    }

    #[test]
    fn playing_twice_over_does_not_lose_the_authored_scene() {
        // The snapshot is taken on the way in. Taking it again while already
        // playing would overwrite the document with whatever the game had done
        // to it by then - which is the worst possible failure here, because it
        // looks like nothing happened until Stop.
        let (dir, mut scene, node) = project();
        let before = scene.to_ron().expect("the scene serializes");

        let mut play = Play::new(dir.path());
        play.start(&mut scene);
        play.step(&mut scene, 0.016);
        play.start(&mut scene);
        play.step(&mut scene, 0.016);
        play.stop(&mut scene);

        assert_eq!(scene.to_ron().expect("the scene serializes"), before);
        assert_eq!(scene.node(node).transform.translation.x, 3.0);
    }
}
