//! Play mode: the editor's half of running a game.
//!
//! The loop itself is `voyager::Runtime` and lives in voyager on purpose (ADR
//! 0002). What is editor-specific, and therefore here, is that pressing Play in
//! an editor is a *rehearsal*: the authored scene is the document, the running
//! game is allowed to scribble on it, and Stop puts the document back. A shipped
//! game has nothing to put back, which is why voyager does not know about any of
//! this.

use std::path::PathBuf;

use helios::{Input, NodeId, Scene, Value};
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

    /// What the script at `(node, component)` currently holds for each of its
    /// exported variables, or `None` if nothing is running there.
    pub fn live_exports(
        &mut self,
        scene: &Scene,
        node: NodeId,
        component: usize,
    ) -> Option<Vec<(String, Value)>> {
        self.runtime.as_mut()?.live_exports(scene, node, component)
    }

    /// Take what the running scripts printed, each line tagged with which
    /// script printed it.
    pub fn take_output(&mut self) -> Vec<String> {
        self.runtime
            .as_mut()
            .map(Runtime::take_output)
            .unwrap_or_default()
    }

    /// Take what went wrong: a script that would not compile when the game
    /// started, or one that trapped mid-frame.
    pub fn take_problems(&mut self) -> Vec<String> {
        self.runtime
            .as_mut()
            .map(Runtime::take_problems)
            .unwrap_or_default()
    }

    /// Set what the scripts will read as input on the next frame.
    ///
    /// Pushed in rather than pulled out, because the host is what knows the
    /// difference between a key that means "left" and a key that means the
    /// letter A. voyager holds the answer; atlas decides it.
    pub fn set_input(&mut self, input: Input) {
        if let Some(runtime) = self.runtime.as_mut() {
            *runtime.input_mut() = input;
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

    use helios::{Component, Node, ScriptComponent};

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

    /// The directory the editor actually opens, and a scene built here that
    /// runs one of the scripts it ships.
    ///
    /// The project's own `scenes/main.ron` is somebody's working file - it gets
    /// dragged around and re-pointed while the editor is open - so a test that
    /// reads it is a test that breaks for reasons that are not bugs. The
    /// *directory* and the *scripts* are what this needs: they are what would
    /// catch a source path resolved against the wrong root.
    fn demo_scene_running(script: &str) -> (std::path::PathBuf, Scene, NodeId, usize) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("demo_project");
        assert!(
            dir.join(script).is_file(),
            "the demo project still ships {script}"
        );
        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("player"));
        scene
            .node_mut(node)
            .components
            .push(Component::Script(ScriptComponent {
                source: script.to_string(),
                ..ScriptComponent::default()
            }));
        (dir, scene, node, 0)
    }

    #[test]
    fn a_script_the_demo_project_ships_actually_plays() {
        // Against the real project directory, so a relative source path
        // resolved against the wrong root fails here rather than in the editor.
        let (dir, mut scene, node, _) = demo_scene_running("scripts/bounce.cmt");
        let before = scene.node(node).transform.translation;

        let mut play = Play::new(&dir);
        play.start(&mut scene);
        assert!(play.is_playing(), "the shipped script compiled");
        for _ in 0..10 {
            play.step(&mut scene, 0.016);
        }

        assert_ne!(
            scene.node(node).transform.translation,
            before,
            "ten frames of a shipped script moved the node"
        );
        play.stop(&mut scene);
        assert_eq!(scene.node(node).transform.translation, before);
    }

    #[test]
    fn a_readonly_export_can_be_watched_while_it_runs() {
        // `travelled` in tunable.cmt is `@readonly` and the script drives it -
        // it exists to be watched. Reading it back is what makes the inspector
        // show the game rather than a snapshot of what it was handed.
        let (dir, mut scene, node, component) = demo_scene_running("scripts/tunable.cmt");
        let Some(Component::Script(script)) = scene.node_mut(node).components.get_mut(component)
        else {
            panic!("the fixture attaches a script");
        };
        script.exports = vec![
            ("speed".to_string(), helios::Value::F32(50.0)),
            (
                "direction".to_string(),
                helios::Value::Vec2(glam::Vec2::new(1.0, 0.0)),
            ),
            ("travelled".to_string(), helios::Value::F32(0.0)),
        ];

        let mut play = Play::new(&dir);
        play.start(&mut scene);
        for _ in 0..10 {
            play.step(&mut scene, 0.016);
        }

        let live = play
            .live_exports(&scene, node, component)
            .expect("the script is running");
        let travelled = live
            .iter()
            .find(|(name, _)| name == "travelled")
            .map(|(_, value)| value.clone())
            .expect("tunable.cmt exports travelled");
        assert!(
            matches!(travelled, helios::Value::F32(x) if x > 0.0),
            "ten frames of the script's own arithmetic: {travelled:?}"
        );

        // The component still holds what it was given, because a readout is not
        // ownership (ADR 0022) - which is what lets Stop put the authored
        // numbers back with nothing to undo.
        let Some(Component::Script(script)) = scene.node(node).components.get(component) else {
            panic!("it is a script component");
        };
        assert_eq!(
            script.exports.iter().find(|(n, _)| n == "travelled"),
            Some(&("travelled".to_string(), helios::Value::F32(0.0)))
        );
    }

    #[test]
    fn on_destroy_sees_the_game_rather_than_the_document() {
        // Teardown runs before the restore, because a script's last word is
        // about the game it was in. Running it after would hand it a scene the
        // game never saw.
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(
            dir.path().join("last.cmt"),
            "func update(dt: f32) { transform.position.x += 10.0; }
             func on_destroy() { print(str(transform.position.x)); }",
        )
        .expect("writing the script");

        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("player"));
        scene
            .node_mut(node)
            .components
            .push(Component::Script(ScriptComponent {
                source: "last.cmt".to_string(),
                ..ScriptComponent::default()
            }));

        let mut play = Play::new(dir.path());
        play.start(&mut scene);
        play.step(&mut scene, 0.016);
        play.stop(&mut scene);

        let output = play.take_output().join(" ");
        assert!(
            output.contains("10"),
            "it saw where the game left it, not where the document says: {output}"
        );
        assert_eq!(
            scene.node(node).transform.translation.x,
            0.0,
            "and the document is still restored afterwards"
        );
    }

    #[test]
    fn a_script_that_will_not_compile_is_a_problem_rather_than_a_refusal_to_play() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(
            dir.path().join("broken.cmt"),
            "func update(dt: f32) { $$$ }",
        )
        .expect("writing the script");

        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("player"));
        scene
            .node_mut(node)
            .components
            .push(Component::Script(ScriptComponent {
                source: "broken.cmt".to_string(),
                ..ScriptComponent::default()
            }));

        let mut play = Play::new(dir.path());
        play.start(&mut scene);
        assert!(play.is_playing(), "the session started anyway");

        let problems = play.take_problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("broken.cmt"), "{problems:?}");
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
