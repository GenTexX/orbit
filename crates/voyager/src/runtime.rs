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
    /// Lines printed by instances that no longer exist.
    ///
    /// A script's `on_destroy` is its last word, and it is said into a store
    /// this drops moments later. Without somewhere for those lines to wait,
    /// the one message a teardown prints is the one message nobody ever sees.
    output: Vec<String>,
    playing: bool,
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
            output: Vec::new(),
            playing: false,
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
        let mut lines = std::mem::take(&mut self.output);
        for running in &mut self.running {
            lines.extend(running.script.take_printed_tagged());
        }
        lines
    }

    /// Take what has gone wrong since this was last called.
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    /// Whether a play session is running.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Start every script in the scene, in scene pre-order.
    ///
    /// Pre-order because it is the order the scene tree is written, read and
    /// drawn in, so it is the order a person predicts. A parent runs before its
    /// children, and siblings run in the order they appear in the panel. There
    /// is no dependency between scripts to honour in Milestone 5 - nothing can
    /// see another node - so the rule is chosen for how legible it is rather
    /// than forced, and it is picked now because changing it later would change
    /// what existing games do.
    ///
    /// A script that will not compile is reported and skipped. Never fatal:
    /// one broken file in a scene of twenty must leave nineteen playable, or a
    /// typo stops being a mistake and starts being an outage. The same goes for
    /// a source file that has been moved or deleted since it was attached.
    ///
    /// Calling this while already playing does nothing. In-process Play leaves
    /// the editing UI live and clickable, so pressing the button twice is a
    /// thing that happens, and the honest answer to "start what is already
    /// started" is nothing - certainly not silently restarting a game somebody
    /// is in the middle of.
    pub fn play(&mut self, scene: &mut Scene) {
        if self.playing {
            return;
        }
        self.playing = true;
        for (node, component, source) in scripts_in_pre_order(scene) {
            if let Err(err) = self.attach(scene, node, component) {
                self.problems.push(format!("[{source}] {err}"));
            }
        }
    }

    /// End the session: every script's `on_destroy`, then nothing is running.
    ///
    /// In the same order they update in. Nothing in a scene can observe another
    /// node yet, so no teardown order is forced, and one rule is easier to hold
    /// in your head than two.
    ///
    /// Restoring the scene to what it was before Play is not done here. That is
    /// the editor's to do, because it is the editor that has a document to put
    /// back; a shipped game stopping is a game exiting.
    pub fn stop(&mut self, scene: &mut Scene) {
        if !self.playing {
            return;
        }
        self.playing = false;
        for mut running in std::mem::take(&mut self.running) {
            if let Err(err) = running.script.destroy(scene, running.node) {
                self.problems
                    .push(format!("[{}] {err}", running.script.label()));
            }
            // Said into a store that is about to be dropped, so it is taken
            // here or it is lost.
            self.output.extend(running.script.take_printed_tagged());
        }
    }

    fn index_of(&self, node: NodeId, component: usize) -> Option<usize> {
        self.running
            .iter()
            .position(|running| running.node == node && running.component == component)
    }
}

/// Every script component in the scene, in pre-order: a node before its
/// children, siblings left to right, and a node's own components in the order
/// they were added.
///
/// Collected rather than walked lazily because starting a script takes the
/// scene mutably - a state initializer can move the node it runs for - so the
/// walk has to be finished before the first instantiation begins.
fn scripts_in_pre_order(scene: &Scene) -> Vec<(NodeId, usize, String)> {
    let mut found = Vec::new();
    let mut stack = vec![scene.root()];
    while let Some(node) = stack.pop() {
        for (component, kind) in scene.node(node).components.iter().enumerate() {
            if let Component::Script(script) = kind {
                found.push((node, component, script.source.clone()));
            }
        }
        // Reversed, because the stack hands them back in the order it pops.
        stack.extend(scene.children(node).iter().rev().copied());
    }
    found
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

    /// Attach a script component naming `source` to `node`.
    fn attach_script(scene: &mut Scene, node: NodeId, source: &str) {
        scene
            .node_mut(node)
            .components
            .push(Component::Script(ScriptComponent {
                source: source.to_string(),
                ..ScriptComponent::default()
            }));
    }

    /// Write a script that prints `name` every frame.
    fn announcer(dir: &Path, name: &str) -> String {
        let file = format!("{name}.cmt");
        std::fs::write(
            dir.join(&file),
            format!("func update(dt: f32) {{ print(\"{name}\"); }}"),
        )
        .expect("writing a script");
        file
    }

    /// The names in the order they were printed, with the `[file]` tag off.
    fn spoken(runtime: &mut Runtime) -> Vec<String> {
        runtime
            .take_output()
            .iter()
            .map(|line| {
                line.rsplit_once("] ")
                    .map_or(line.clone(), |(_, said)| said.to_string())
            })
            .collect()
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

    #[test]
    fn play_starts_every_script_in_scene_pre_order() {
        // The order a person predicts is the order the panel shows: a parent
        // before its children, siblings top to bottom, and a node's own
        // components in the order they were added. Nothing in M5 depends on
        // one script running before another, which is exactly why the rule has
        // to be picked and pinned now - once a game relies on it, changing it
        // breaks that game silently.
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut scene = Scene::new("root");
        let a = scene.add_child(scene.root(), Node::new("a"));
        let child = scene.add_child(a, Node::new("a-child"));
        let b = scene.add_child(scene.root(), Node::new("b"));

        // The root carries one too: it is a node like any other.
        let root = scene.root();
        attach_script(&mut scene, root, &announcer(dir.path(), "root"));
        // Two on one node, because a node may run several scripts.
        attach_script(&mut scene, a, &announcer(dir.path(), "a-first"));
        attach_script(&mut scene, a, &announcer(dir.path(), "a-second"));
        attach_script(&mut scene, child, &announcer(dir.path(), "a-child"));
        attach_script(&mut scene, b, &announcer(dir.path(), "b"));

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.play(&mut scene);
        assert!(runtime.is_playing());
        assert_eq!(runtime.len(), 5, "every script, including the root's");

        runtime.step(&mut scene, 0.016);
        assert_eq!(
            spoken(&mut runtime),
            ["root", "a-first", "a-second", "a-child", "b"],
            "depth first, siblings in order, components in order"
        );
    }

    #[test]
    fn a_script_that_will_not_compile_is_reported_and_the_rest_still_play() {
        // One broken file in a scene of twenty has to leave nineteen playable,
        // or a typo stops being a mistake and becomes an outage.
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(
            dir.path().join("broken.cmt"),
            "func update(dt: f32) { $$$ }",
        )
        .expect("writing the broken script");

        let mut scene = Scene::new("root");
        let broken = scene.add_child(scene.root(), Node::new("broken"));
        let fine = scene.add_child(scene.root(), Node::new("fine"));
        attach_script(&mut scene, broken, "broken.cmt");
        attach_script(&mut scene, fine, &announcer(dir.path(), "fine"));

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.play(&mut scene);

        assert_eq!(runtime.len(), 1, "the broken one is skipped, not fatal");
        let problems = runtime.take_problems();
        assert_eq!(problems.len(), 1, "and reported: {problems:?}");
        assert!(
            problems[0].contains("broken.cmt"),
            "naming the file, since the message is about a file: {problems:?}"
        );

        runtime.step(&mut scene, 0.016);
        assert_eq!(spoken(&mut runtime), ["fine"], "the good one plays");
    }

    #[test]
    fn a_source_file_that_is_gone_is_reported_rather_than_fatal() {
        // Same policy, different failure: the component still names a file the
        // project no longer has. Deleting a script in the explorer must not
        // make Play unusable.
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("ghost"));
        attach_script(&mut scene, node, "vanished.cmt");

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.play(&mut scene);

        assert!(runtime.is_playing(), "the session still started");
        assert!(runtime.is_empty());
        let problems = runtime.take_problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("vanished.cmt"), "{problems:?}");
    }

    #[test]
    fn stop_runs_on_destroy_and_keeps_its_last_words() {
        // `on_destroy` is a script's last word and it is said into a store that
        // is dropped moments later. Without somewhere for the line to wait, the
        // one message a teardown prints is the one nobody ever sees.
        let (dir, mut scene, _node) = project(
            "func update(dt: f32) { transform.position.x += 1.0; }
             func on_destroy() { print(\"bye\"); }",
        );
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");

        runtime.play(&mut scene);
        runtime.step(&mut scene, 0.016);
        assert!(
            spoken(&mut runtime).is_empty(),
            "nothing said while running"
        );

        runtime.stop(&mut scene);
        assert!(!runtime.is_playing());
        assert!(runtime.is_empty(), "and nothing is left running");
        assert_eq!(spoken(&mut runtime), ["bye"]);
    }

    #[test]
    fn stopping_and_playing_again_is_a_fresh_run() {
        // Play is a rehearsal that can be run twice. The second one gets new
        // instances, so `start` runs again and whatever the first run
        // accumulated is gone.
        let (dir, mut scene, node) = project(
            "let count = 0.0;
             func start() { count = 0.0; }
             func update(dt: f32) { count += 1.0; transform.position.x = count; }",
        );
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");

        runtime.play(&mut scene);
        for _ in 0..3 {
            runtime.step(&mut scene, 0.016);
        }
        assert_eq!(x(&scene, node), 3.0);

        runtime.stop(&mut scene);
        runtime.play(&mut scene);
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 1.0, "counting from the start again");
    }

    #[test]
    fn pressing_play_while_playing_does_nothing() {
        // In-process Play leaves the editing UI live and clickable, so this
        // happens. Silently restarting a game somebody is in the middle of is
        // the wrong answer to it.
        let (dir, mut scene, node) = project(
            "let count = 0.0;
             func update(dt: f32) { count += 1.0; transform.position.x = count; }",
        );
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");

        runtime.play(&mut scene);
        runtime.step(&mut scene, 0.016);
        runtime.play(&mut scene);
        runtime.step(&mut scene, 0.016);

        assert_eq!(runtime.len(), 1, "one instance, not two");
        assert_eq!(x(&scene, node), 2.0, "and it kept counting");
    }

    #[test]
    fn stopping_when_nothing_is_playing_does_nothing() {
        let (dir, mut scene, _node) = project("func on_destroy() { print(\"bye\"); }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.stop(&mut scene);
        assert!(!runtime.is_playing());
        assert!(spoken(&mut runtime).is_empty(), "nothing to tear down");
    }
}
