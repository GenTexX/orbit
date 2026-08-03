//! The thing that owns running scripts and steps them.
//!
//! This lives in voyager rather than in the editor because of ADR 0002: the
//! editor links the runtime as a library and a shipped game is a thin wrapper
//! around the same library. Everything here would fit into `atlas::State`
//! easily, which is precisely the reason it must not go there - a loop that
//! only the editor can run is a loop a game cannot.
//!
//! It depends on helios and nothing else. Stepping a scene is not drawing one,
//! so photon stays out of this crate's dependency list on purpose.

use std::path::{Path, PathBuf};

use helios::{Component, Input, NodeId, Scene, ScriptError, ScriptHost, ScriptInstance};

/// One script instance, and the address in the scene it belongs to.
struct Running {
    node: NodeId,
    /// Which of the node's components this instance is running for.
    ///
    /// A node may carry several scripts, so the node alone does not identify an
    /// instance. The pair is the key, and the position in [`Runtime::running`]
    /// is the update order.
    component: usize,
    script: ScriptInstance,
}

/// Owns every running script, and steps them.
///
/// The instance "map" is a `Vec` because the order is part of the data: scripts
/// update in the order they were attached, and a `HashMap` would make that
/// order the iteration order of a hash. Lookup by key is a scan, which is the
/// right trade at the scale a scene has - a scene with a thousand scripts has a
/// bigger problem than this scan.
pub struct Runtime {
    host: ScriptHost,
    /// The project directory, which the components' relative source paths are
    /// resolved against.
    root: PathBuf,
    running: Vec<Running>,
    input: Input,
    /// What went wrong, waiting for whoever shows a console.
    ///
    /// Collected rather than returned: a script failing is not the caller's
    /// error, it is news. The frame carries on, the other scripts still run,
    /// and the host decides when to read this.
    problems: Vec<String>,
}

impl Runtime {
    /// A runtime for the project rooted at `root`, with nothing running yet.
    ///
    /// Fails only if wasmtime cannot be brought up at all, which is a broken
    /// installation rather than a broken script.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ScriptError> {
        Ok(Runtime {
            host: ScriptHost::new()?,
            root: root.into(),
            running: Vec::new(),
            input: Input::default(),
            problems: Vec::new(),
        })
    }

    /// The input every script will read this frame.
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// The input, to be written by whoever is watching the keyboard.
    pub fn input_mut(&mut self) -> &mut Input {
        &mut self.input
    }

    /// The project directory relative script paths are resolved against.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// How many scripts are running.
    pub fn len(&self) -> usize {
        self.running.len()
    }

    /// Whether nothing is running.
    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    /// Start the script component at `(node, component)`, running its `start`.
    ///
    /// Two things are deliberately not errors. A component index that is not a
    /// script attaches nothing, and a script component whose `source` is still
    /// empty attaches nothing - that is what Add Script leaves behind until a
    /// file is picked, and a node in that state is half-built rather than
    /// broken.
    ///
    /// An instance already at this address is replaced *in place*, keeping its
    /// position in the update order. That is what a hot reload needs: swapping
    /// a script must not quietly move it to the end of the frame.
    ///
    /// The compile failure comes back to the caller rather than into
    /// [`problems`](Self::take_problems), because the caller is the one that
    /// knows whether this attach was a whole scene starting (report it and keep
    /// going) or a single reload (report it and keep the old instance).
    pub fn attach(
        &mut self,
        scene: &mut Scene,
        node: NodeId,
        component: usize,
    ) -> Result<(), ScriptError> {
        let Some(Component::Script(script)) = scene.node(node).components.get(component) else {
            return Ok(());
        };
        if script.source.is_empty() {
            return Ok(());
        }
        // Copied out before instantiating: starting a script takes the scene
        // mutably, because a state initializer can move the node it runs for.
        let path = self.root.join(&script.source);
        let exports = script.exports.clone();

        let instance = self.host.instantiate_file(&path, scene, node, &exports)?;
        let slot = Running {
            node,
            component,
            script: instance,
        };
        match self.index_of(node, component) {
            Some(at) => self.running[at] = slot,
            None => self.running.push(slot),
        }
        Ok(())
    }

    /// Run one frame: every script's `update(dt)`, in attach order.
    ///
    /// A script that traps is recorded and the frame carries on. One broken
    /// script must not stop the other nineteen - and the instance latches
    /// itself as stopped, so this reports it once rather than sixty times a
    /// second.
    pub fn step(&mut self, scene: &mut Scene, dt: f32) {
        for running in &mut self.running {
            if let Err(err) = running.script.update(scene, running.node, dt) {
                self.problems
                    .push(format!("[{}] {err}", running.script.label()));
            }
        }
    }

    /// Take everything the running scripts have printed, tagged with which
    /// script printed it - since `print("hi")` from two nodes is otherwise two
    /// identical lines.
    pub fn take_output(&mut self) -> Vec<String> {
        self.running
            .iter_mut()
            .flat_map(|running| running.script.take_printed_tagged())
            .collect()
    }

    /// Take what has gone wrong since this was last called.
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    fn index_of(&self, node: NodeId, component: usize) -> Option<usize> {
        self.running
            .iter()
            .position(|running| running.node == node && running.component == component)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use helios::{Node, ScriptComponent, Value};

    /// A project directory with `script.cmt` in it, plus a scene whose single
    /// child node runs it.
    fn project(source: &str) -> (tempfile::TempDir, Scene, NodeId) {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("script.cmt"), source).expect("writing the script");

        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("player"));
        scene
            .node_mut(node)
            .components
            .push(Component::Script(ScriptComponent {
                source: "script.cmt".to_string(),
                ..ScriptComponent::default()
            }));
        (dir, scene, node)
    }

    fn x(scene: &Scene, node: NodeId) -> f32 {
        scene.node(node).transform.translation.x
    }

    #[test]
    fn a_stepped_script_moves_the_node_it_runs_for() {
        // The whole point of the crate in one test: a runtime, a scene, a
        // frame, and a transform that is somewhere else afterwards.
        let (dir, mut scene, node) =
            project("func update(dt: f32) { transform.position.x += 10.0; }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");

        runtime.attach(&mut scene, node, 0).expect("it compiles");
        assert_eq!(runtime.len(), 1);
        assert_eq!(x(&scene, node), 0.0, "attaching alone is not a frame");

        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 10.0);
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 20.0, "and state carries between frames");
    }

    #[test]
    fn start_runs_at_attach_and_never_again() {
        // `start` is where a script captures where it was placed. Running it
        // twice would teleport the node back, which is the failure hot reload
        // has to avoid later - so it is worth pinning here, where the
        // lifecycle is simple enough to see.
        let (dir, mut scene, node) = project(
            "let home = vec2(0.0, 0.0);
             func start() { home = transform.position; }
             func update(dt: f32) { transform.position.x = home.x + 5.0; }",
        );
        scene.node_mut(node).transform.translation.x = 100.0;

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        for _ in 0..3 {
            runtime.step(&mut scene, 0.016);
        }
        assert_eq!(
            x(&scene, node),
            105.0,
            "home was captured once, at the position the node was placed at"
        );
    }

    #[test]
    fn the_inspectors_values_win_over_the_scripts_defaults() {
        // ADR 0022: the initializer in the source is the default, and the
        // component holds what the user tuned. The runtime is what carries the
        // second one across.
        let (dir, mut scene, node) = project(
            "@export let speed: f32 = 1.0;
             func update(dt: f32) { transform.position.x += speed; }",
        );
        let Some(Component::Script(script)) = scene.node_mut(node).components.get_mut(0) else {
            panic!("the fixture attaches a script");
        };
        script.exports = vec![("speed".to_string(), Value::F32(7.0))];

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 7.0, "the tuned value, not the default 1.0");
    }

    #[test]
    fn a_script_that_traps_is_reported_once_and_the_frame_carries_on() {
        let (dir, mut scene, node) = project("func update(dt: f32) { while true { } }");
        let good = scene.add_child(scene.root(), Node::new("other"));
        scene
            .node_mut(good)
            .components
            .push(Component::Script(ScriptComponent {
                source: "good.cmt".to_string(),
                ..ScriptComponent::default()
            }));
        std::fs::write(
            dir.path().join("good.cmt"),
            "func update(dt: f32) { transform.position.x += 1.0; }",
        )
        .expect("writing the second script");

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        runtime
            .attach(&mut scene, good, 0)
            .expect("so does the other");

        runtime.step(&mut scene, 0.016);
        assert_eq!(
            x(&scene, good),
            1.0,
            "one script trapping must not cost the others their frame"
        );
        let problems = runtime.take_problems();
        assert_eq!(problems.len(), 1, "reported: {problems:?}");
        assert!(
            problems[0].contains("loop with no way out"),
            "and reported usefully: {problems:?}"
        );

        // Latched, so a console does not fill with the same line sixty times a
        // second - and the good script keeps going.
        runtime.step(&mut scene, 0.016);
        assert!(runtime.take_problems().is_empty(), "said once");
        assert_eq!(x(&scene, good), 2.0);
    }

    #[test]
    fn printed_lines_come_back_tagged_and_drained() {
        let (dir, mut scene, node) = project("func update(dt: f32) { print(\"tick\"); }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");

        runtime.step(&mut scene, 0.016);
        let output = runtime.take_output();
        assert_eq!(output.len(), 1);
        assert!(
            output[0].contains("script.cmt") && output[0].contains("tick"),
            "which script said it, and what it said: {output:?}"
        );
        assert!(runtime.take_output().is_empty(), "and taking empties it");
    }

    #[test]
    fn a_component_with_no_script_yet_attaches_nothing_and_is_not_an_error() {
        // What "Add Script" leaves behind until a file is picked. A node in
        // that state is half-built, not broken, and pressing Play on it must
        // not produce a console error the user cannot act on.
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("player"));
        scene
            .node_mut(node)
            .components
            .push(Component::Script(ScriptComponent::default()));

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime
            .attach(&mut scene, node, 0)
            .expect("an empty source is not a failure");
        assert!(runtime.is_empty());

        // Nor is an index that is not a script at all.
        runtime
            .attach(&mut scene, node, 7)
            .expect("nothing at that address is nothing to do");
        assert!(runtime.is_empty());
    }

    #[test]
    fn attaching_again_replaces_in_place_and_keeps_the_order() {
        // What the reload swap will need: a script that is replaced must not
        // move to the end of the frame. Two scripts that both write the same
        // field make the order visible - last writer wins.
        let (dir, mut scene, first) =
            project("func update(dt: f32) { transform.position.x = 1.0; }");
        std::fs::write(
            dir.path().join("second.cmt"),
            "func update(dt: f32) { transform.position.x = 2.0; }",
        )
        .expect("writing the second script");
        scene
            .node_mut(first)
            .components
            .push(Component::Script(ScriptComponent {
                source: "second.cmt".to_string(),
                ..ScriptComponent::default()
            }));

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, first, 0).expect("it compiles");
        runtime
            .attach(&mut scene, first, 1)
            .expect("so does the second");
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, first), 2.0, "the second component runs last");

        // Re-attaching the first must not push it behind the second.
        runtime.attach(&mut scene, first, 0).expect("it recompiles");
        assert_eq!(runtime.len(), 2, "replaced, not added");
        runtime.step(&mut scene, 0.016);
        assert_eq!(
            x(&scene, first),
            2.0,
            "and it is still the one that runs first"
        );
    }
}
