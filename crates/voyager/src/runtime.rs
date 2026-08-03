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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use helios::{
    Begin, Component, FRAME_BUDGET, Input, NodeId, Scene, ScriptError, ScriptHost, ScriptInstance,
    Value,
};

/// One script instance, and the address in the scene it belongs to.
struct Running {
    node: NodeId,
    /// The file this instance was compiled from, as the component names it -
    /// project-relative, so it reads the way the inspector shows it.
    relative: String,
    /// Which of the node's components this instance is running for.
    ///
    /// A node may carry several scripts, so the node alone does not identify an
    /// instance. The pair is the key, and the position in [`Runtime::running`]
    /// is the update order.
    component: usize,
    script: ScriptInstance,
}

/// The most a whole frame may spend on scripts before the rest are deferred.
///
/// helios enforces a budget per *call*, so twenty runaway scripts cost twenty
/// times it - measured at two seconds for one `step`, which is the editor gone
/// with an unsaved scene in it. This bounds the pile-up.
///
/// Deliberately several call budgets rather than one. At exactly one, a single
/// runaway would eat the whole frame and every other script would be skipped -
/// which trades a bounded cost for a broken guarantee, and the guarantee is the
/// one this runtime is built on: one bad script must not cost the other
/// nineteen their frame. At four, a runaway still costs only its own budget,
/// the good scripts around it still run, and the worst case is four budgets
/// rather than twenty. Traps latch, so the frame after is clean either way.
const FRAME_LIMIT: Duration = Duration::from_millis(FRAME_BUDGET.as_millis() as u64 * 4);

/// What one script is doing, for anything that wants to show a play session.
///
/// The runtime knew all of this and none of it was reachable: `Running` is
/// private and `Runtime` exposed only a count, so a script that trapped was
/// indistinguishable from one with nothing to do - the game looked like it was
/// playing while one node quietly sat still forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceInfo {
    pub node: NodeId,
    pub component: usize,
    /// The source file, as the component names it - project-relative, so it
    /// reads the way the inspector shows it.
    pub source: String,
    /// False once a trap has latched it off. It will not be called again until
    /// a reload replaces it.
    pub running: bool,
}

/// Something the runtime wants a console to say, with what an editor needs to
/// act on it kept rather than flattened into the sentence.
///
/// helios goes to real trouble producing this: a compile failure carries every
/// diagnostic with its span, and a trap carries the comet function lifted out
/// of the wasm backtrace. Turning all of it into a `String` here made the
/// errors from actually running the game the only ones in the editor that could
/// not open a file or squiggle a line.
#[derive(Debug, Clone)]
pub struct Problem {
    /// The script it happened in, project-relative, or empty for a problem that
    /// belongs to the frame rather than to one script.
    pub source: String,
    /// The comet function a trap happened in, when the module's name section
    /// named it. An editor can put a caret here.
    pub function: Option<String>,
    /// Every diagnostic, spans included, when this was a compile failure.
    pub diagnostics: Vec<comet::Diagnostic>,
    pub message: String,
}

impl std::fmt::Display for Problem {
    /// The line a shipped game prints, and the one the console showed before
    /// any of the structure above existed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.source.is_empty() {
            true => write!(f, "{}", self.message),
            false => write!(f, "[{}] {}", self.source, self.message),
        }
    }
}

impl Problem {
    fn from_error(source: &str, err: &ScriptError) -> Self {
        Problem {
            source: source.to_string(),
            function: err.trapped_in().map(str::to_string),
            diagnostics: match err {
                ScriptError::Compile(diagnostics) => diagnostics.clone(),
                _ => Vec::new(),
            },
            message: err.to_string(),
        }
    }

    /// A problem that belongs to the frame rather than to one script.
    fn frame(message: String) -> Self {
        Problem {
            source: String::new(),
            function: None,
            diagnostics: Vec::new(),
            message,
        }
    }
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
    problems: Vec<Problem>,
    /// When each running script's source file was last written, as of the last
    /// time anybody looked.
    ///
    /// Keyed by the resolved absolute path, so two nodes running one file share
    /// an entry - a save is one change however many instances it affects.
    ///
    /// An mtime poll rather than a filesystem watcher, following the precedent
    /// the theme already set: a handful of `stat` calls a few times a second
    /// costs nothing measurable, needs no new dependency, and cannot leak a
    /// watch handle. It also works the same on every platform, which a watcher
    /// famously does not.
    watched: HashMap<PathBuf, Option<SystemTime>>,
    /// Every `(node, component)` this session has tried to bring up, including
    /// the ones that would not compile. `running` holds only the successes, so
    /// without this a reload has no way to find the address that needs one
    /// most.
    attached: std::collections::HashSet<(NodeId, usize)>,
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
            watched: HashMap::new(),
            attached: std::collections::HashSet::new(),
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
        self.bring_up(scene, node, component, Begin::Fresh)
    }

    fn bring_up(
        &mut self,
        scene: &mut Scene,
        node: NodeId,
        component: usize,
        begin: Begin,
    ) -> Result<(), ScriptError> {
        let Some(Component::Script(script)) = scene.node(node).components.get(component) else {
            return Ok(());
        };
        if script.source.is_empty() {
            return Ok(());
        }
        // Copied out before instantiating: starting a script takes the scene
        // mutably, because a state initializer can move the node it runs for.
        let relative = script.source.clone();
        let path = self.root.join(&relative);
        let exports = script.exports.clone();

        // Watched before it is compiled, not after. A file that does not
        // compile when Play is pressed is exactly the file somebody is about to
        // fix, and watching only what succeeded meant the fix was never noticed
        // - press Play, read the error, fix the typo, save, and nothing
        // happened. This reverses a rule that used to be written here.
        self.watched
            .entry(path.clone())
            .or_insert_with(|| modified(&path));
        self.attached.insert((node, component));

        let instance = self
            .host
            .instantiate_file(&path, scene, node, &exports, begin)?;
        self.watched.insert(path.clone(), modified(&path));
        let slot = Running {
            node,
            relative,
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
        let deadline = Instant::now() + FRAME_LIMIT;
        let mut skipped = 0usize;
        for running in &mut self.running {
            if Instant::now() >= deadline {
                skipped += 1;
                continue;
            }
            // Every script sees the same input this frame, set before the call
            // rather than read during it: a frame's input is a fixed thing
            // while that frame runs.
            running.script.set_input(self.input);
            if let Err(err) = running.script.update(scene, running.node, dt) {
                self.problems
                    .push(Problem::from_error(&running.relative, &err));
            }
        }
        // Said once with a count rather than once per script, because a frame
        // that ran out of time has nothing useful to say about the twentieth
        // script it did not reach.
        if skipped > 0 {
            let budget = FRAME_LIMIT.as_millis();
            self.problems.push(Problem::frame(format!(
                "this frame ran out of its {budget}ms before {skipped} more scripts \
                 could run - something in this scene is far too slow"
            )));
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

    /// What the script at `(node, component)` currently holds for each of its
    /// exported variables, or `None` if nothing is running there.
    ///
    /// A readout of the module, not of the component. The component owns these
    /// values (ADR 0022) and keeps owning them; this answers the other
    /// question - what does the game have right now - which stops being the
    /// same question the moment a script assigns to one of its own exports.
    pub fn live_exports(
        &mut self,
        scene: &Scene,
        node: NodeId,
        component: usize,
    ) -> Option<Vec<(String, Value)>> {
        let Some(Component::Script(script)) = scene.node(node).components.get(component) else {
            return None;
        };
        let declared = script.exports.clone();
        let at = self.index_of(node, component)?;
        Some(self.running[at].script.read_exports(&declared))
    }

    /// Which running scripts' source files have been written since they were
    /// last read.
    ///
    /// Reporting a change also *accepts* it: the same save is never offered
    /// twice, even if acting on it fails. A file that will not compile would
    /// otherwise be retried sixty times a second, and the console would fill
    /// with one syntax error while the person was still typing the line.
    ///
    /// A file that cannot be stat-ed at all - deleted, or renamed out from
    /// under a running game - reads as unchanged. Deleting a script is not a
    /// request to reload it, and there would be nothing to reload it from; the
    /// instance keeps running what it was given, which is the same answer a
    /// broken save gets.
    pub fn changed_sources(&mut self) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        for (path, seen) in &mut self.watched {
            let Some(now) = modified(path) else {
                continue;
            };
            if *seen != Some(now) {
                *seen = Some(now);
                changed.push(path.clone());
            }
        }
        // The map's iteration order is a hash's, and a caller acting on this
        // list should not depend on one. Sorted so a reload of two files is the
        // same reload twice.
        changed.sort();
        changed
    }

    /// Swap in the current contents of `path` under every instance running it.
    ///
    /// The four steps are the whole of hot reload: forget the compiled module so
    /// the file is read again, recompile it, instantiate with the values the
    /// component holds, and put the new instance in the old one's place.
    ///
    /// **A source that no longer compiles leaves the old instance running** and
    /// reports. That is the decision the rest of this is arranged around: saving
    /// a file mid-thought is how people work, and a game that stops dead on a
    /// half-typed line is a game nobody can iterate on. The old module stays
    /// alive because the running instance holds it, so the game keeps playing
    /// the last version that worked until one compiles again.
    ///
    /// **`start` does not run** (see [`Begin`]). A script whose `start` places
    /// its node would teleport it back on every save.
    ///
    /// **The component's values win, not the running module's.** ADR 0022 makes
    /// the component the owner; a script that has been assigning to its own
    /// exported variable loses what it made of them. That is written down rather
    /// than discovered - it is the same rule the inspector follows everywhere
    /// else, and the alternative is a reload that quietly disagrees with the
    /// panel next to it.
    ///
    /// The instance keeps its position in the update order, because
    /// [`attach`](Self::attach) replaces in place.
    pub fn reload(&mut self, scene: &mut Scene, path: &Path) {
        // Forgotten first, so the recompile below reads the file rather than
        // the module cached from before the save.
        self.host.forget(path);
        // From what was attached rather than from what is running: an instance
        // that failed to compile is in neither `running` nor anywhere else, and
        // it is the one whose reload matters most.
        let mut targets: Vec<(NodeId, usize)> = self
            .attached
            .iter()
            .copied()
            .filter(|&(node, component)| {
                matches!(
                    scene.node(node).components.get(component),
                    Some(Component::Script(script)) if self.root.join(&script.source) == path
                )
            })
            .collect();
        // A set has no order and a reload of two nodes should be the same
        // reload twice.
        targets.sort_by_key(|&(node, component)| (self.index_of(node, component), component));
        for (node, component) in targets {
            if let Err(err) = self.bring_up(scene, node, component, Begin::Reload) {
                let relative = path
                    .strip_prefix(&self.root)
                    .unwrap_or(path)
                    .display()
                    .to_string();
                self.problems.push(Problem::from_error(&relative, &err));
            }
        }
    }

    /// Take what has gone wrong since this was last called.
    pub fn take_problems(&mut self) -> Vec<Problem> {
        std::mem::take(&mut self.problems)
    }

    /// What every script in this session is doing.
    pub fn instances(&self) -> Vec<InstanceInfo> {
        self.running
            .iter()
            .map(|running| InstanceInfo {
                node: running.node,
                component: running.component,
                source: running.relative.clone(),
                running: !running.script.is_stopped(),
            })
            .collect()
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
                self.problems.push(Problem::from_error(&source, &err));
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
        self.watched.clear();
        self.attached.clear();
        for mut running in std::mem::take(&mut self.running) {
            if let Err(err) = running.script.destroy(scene, running.node) {
                self.problems
                    .push(Problem::from_error(&running.relative, &err));
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

/// When `path` was last written, or `None` if it cannot be read at all.
fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
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

    use helios::{Node, ScriptComponent};

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
            problems[0].to_string().contains("loop with no way out"),
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
            problems[0].to_string().contains("broken.cmt"),
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
        assert!(
            problems[0].to_string().contains("vanished.cmt"),
            "{problems:?}"
        );
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

    #[test]
    fn a_running_scripts_own_exports_can_be_read_back_live() {
        // What makes an inspector useful during a game: a value the script is
        // driving reads out where it is now, not where it started.
        let (dir, mut scene, node) = project(
            "@export let travelled: f32 = 0.0;
             func update(dt: f32) { travelled = travelled + 1.0; }",
        );
        let Some(Component::Script(script)) = scene.node_mut(node).components.get_mut(0) else {
            panic!("the fixture attaches a script");
        };
        script.exports = vec![("travelled".to_string(), Value::F32(0.0))];

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        runtime.step(&mut scene, 0.016);
        runtime.step(&mut scene, 0.016);

        assert_eq!(
            runtime.live_exports(&scene, node, 0),
            Some(vec![("travelled".to_string(), Value::F32(2.0))]),
            "two frames of the script's own arithmetic"
        );
        // And the component still owns its copy - the readout did not write
        // back, which is what keeps ADR 0022 true.
        let Some(Component::Script(script)) = scene.node(node).components.first() else {
            panic!("the fixture attaches a script");
        };
        assert_eq!(script.exports[0].1, Value::F32(0.0));
    }

    #[test]
    fn nothing_running_at_an_address_has_no_live_exports() {
        let (dir, mut scene, node) = project("func update(dt: f32) { }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        assert_eq!(runtime.live_exports(&scene, node, 0), None);
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        assert_eq!(
            runtime.live_exports(&scene, node, 0),
            Some(Vec::new()),
            "a script with no exports has none, which is not the same as none running"
        );
    }

    /// The player script from the Milestone 5 plan, unchanged except for the
    /// ground line - the north star, in the syntax that plan assumed.
    const PLAYER: &str = "
        @export let speed: f32 = 220.0;
        @export let jump: f32 = 520.0;
        @export let gravity: f32 = 1400.0;
        let vy = 0.0;
        let grounded = false;

        func update(dt: f32) {
            let vx = 0.0;
            if input.left { vx -= speed; }
            if input.right { vx += speed; }
            if input.jump && grounded {
                vy = -jump;
                grounded = false;
            }
            vy += gravity * dt;
            transform.position.x += vx * dt;
            transform.position.y += vy * dt;
            if transform.position.y > 400.0 {
                transform.position.y = 400.0;
                vy = 0.0;
                grounded = true;
            }
        }
    ";

    #[test]
    fn the_player_script_runs_and_jumps() {
        // Milestone 5's proof point, the way "a node moves" was Milestone 4's.
        // When this plan was written the script produced three diagnostics, all
        // of them `cannot find 'input' in this scope`, and input was named as
        // the only language work the milestone needed. This is that work
        // arriving.
        let (dir, mut scene, node) = project(PLAYER);
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");

        // Falling, with nothing held.
        for _ in 0..10 {
            runtime.step(&mut scene, 0.016);
        }
        assert!(x(&scene, node).abs() < f32::EPSILON, "nothing pushed it");
        let fell = scene.node(node).transform.translation.y;
        assert!(fell > 0.0, "gravity: {fell}");

        // Land, then walk right.
        for _ in 0..60 {
            runtime.step(&mut scene, 0.016);
        }
        let standing = scene.node(node).transform.translation.y;
        assert_eq!(standing, 400.0, "it came to rest on the ground line");

        runtime.input_mut().right = true;
        for _ in 0..10 {
            runtime.step(&mut scene, 0.016);
        }
        let walked = x(&scene, node);
        assert!(walked > 30.0, "it walked right: {walked}");

        // Let go and it stops where it is - polled state, not events.
        runtime.input_mut().right = false;
        runtime.step(&mut scene, 0.016);
        assert!(
            (x(&scene, node) - walked).abs() < f32::EPSILON,
            "releasing the key stops it"
        );

        // Jump: grounded, so it leaves the floor.
        runtime.input_mut().jump = true;
        runtime.step(&mut scene, 0.016);
        let airborne = scene.node(node).transform.translation.y;
        assert!(airborne < 400.0, "it left the ground: {airborne}");

        // Held, not re-pressed: the script sees `input.jump` true on every
        // frame, so it bounces the moment it lands again. That is what polled
        // state means, and it is the answer to M5's first open question sitting
        // in a test rather than in prose.
        let mut left_the_ground_again = false;
        for _ in 0..200 {
            runtime.step(&mut scene, 0.016);
            let y = scene.node(node).transform.translation.y;
            if y == 400.0 {
                // Landed. One more frame and a held jump takes it back up.
                runtime.step(&mut scene, 0.016);
                left_the_ground_again = scene.node(node).transform.translation.y < 400.0;
                break;
            }
        }
        assert!(left_the_ground_again, "a held jump bounces");

        // Let go and it settles.
        runtime.input_mut().jump = false;
        for _ in 0..200 {
            runtime.step(&mut scene, 0.016);
        }
        assert_eq!(
            scene.node(node).transform.translation.y,
            400.0,
            "released, it stays on the ground"
        );
    }

    #[test]
    fn a_script_that_writes_to_its_input_is_refused_rather_than_ignored() {
        // The input is the host's answer to what the player is doing, so a
        // script writing to it would be telling the keyboard what was pressed.
        // This used to compile, emit a setter, and be dropped by the host
        // without a word - the schema now says read-only and the checker says
        // so out loud.
        let (dir, mut scene, node) = project(
            "func update(dt: f32) {
                 input.jump = true;
                 if input.jump { transform.position.x += 1.0; }
             }",
        );
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        let err = runtime
            .attach(&mut scene, node, 0)
            .expect_err("it must not compile");
        assert!(
            err.to_string().contains("read-only"),
            "and it says why: {err}"
        );
        assert_eq!(x(&scene, node), 0.0, "nothing ran");
    }

    #[test]
    fn the_mouse_arrives_where_the_script_can_use_it() {
        let (dir, mut scene, node) =
            project("func update(dt: f32) { transform.position = input.mouse; }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        runtime.input_mut().mouse = helios::Vec2::new(12.0, 34.0);
        runtime.step(&mut scene, 0.016);
        assert_eq!(
            scene.node(node).transform.translation,
            helios::Vec2::new(12.0, 34.0)
        );
    }

    /// Rewrite `script.cmt` in a project directory and make sure its mtime
    /// really moved.
    ///
    /// Filesystem timestamps are coarser than a test is fast - some report
    /// whole seconds - so writing twice in a row can leave the same mtime and
    /// make a poll test pass or fail for reasons that have nothing to do with
    /// the code. Bumping it explicitly is exact and takes no time.
    fn rewrite(dir: &Path, name: &str, source: &str) {
        let path = dir.join(name);
        std::fs::write(&path, source).expect("rewriting the script");
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("opening the script");
        file.set_modified(later).expect("bumping the mtime");
    }

    #[test]
    fn a_saved_script_shows_up_as_a_changed_source() {
        let (dir, mut scene, node) = project("func update(dt: f32) { }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        assert!(
            runtime.changed_sources().is_empty(),
            "nothing has been written since it started"
        );

        rewrite(dir.path(), "script.cmt", "func update(dt: f32) { }");
        assert_eq!(
            runtime.changed_sources(),
            vec![dir.path().join("script.cmt")]
        );
    }

    #[test]
    fn the_same_save_is_never_offered_twice() {
        // Reporting a change accepts it. A file that will not compile would
        // otherwise be retried every poll, and the console would fill with one
        // syntax error while somebody was still typing the line.
        let (dir, mut scene, node) = project("func update(dt: f32) { }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");

        rewrite(dir.path(), "script.cmt", "func update(dt: f32) { $$$ }");
        assert_eq!(runtime.changed_sources().len(), 1);
        assert!(runtime.changed_sources().is_empty(), "said once");
    }

    #[test]
    fn a_deleted_script_is_not_a_change() {
        // Deleting is not a request to reload, and there would be nothing to
        // reload from. The instance keeps running what it was given.
        let (dir, mut scene, node) = project("func update(dt: f32) { }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");

        std::fs::remove_file(dir.path().join("script.cmt")).expect("deleting it");
        assert!(runtime.changed_sources().is_empty());
    }

    #[test]
    fn two_nodes_running_one_file_share_one_change() {
        // A save is one change however many instances it affects, which is what
        // keys the watch by path rather than by instance.
        let (dir, mut scene, first) = project("func update(dt: f32) { }");
        let second = scene.add_child(scene.root(), Node::new("other"));
        attach_script(&mut scene, second, "script.cmt");

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, first, 0).expect("it compiles");
        runtime
            .attach(&mut scene, second, 0)
            .expect("so does the other");
        assert_eq!(runtime.len(), 2);

        rewrite(dir.path(), "script.cmt", "func update(dt: f32) { }");
        assert_eq!(runtime.changed_sources().len(), 1);
    }

    #[test]
    fn a_broken_save_does_not_stop_the_next_one_being_noticed() {
        // The sequence that matters. Save something broken, then fix it: the
        // broken save is accepted so it is not retried every poll, and the fix
        // is a fresh change, so whatever acts on this gets its second chance.
        let (dir, mut scene, node) = project("func update(dt: f32) { }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");

        rewrite(dir.path(), "script.cmt", "func update(dt: f32) { $$$ }");
        assert_eq!(runtime.changed_sources().len(), 1, "the broken save");
        assert!(runtime.changed_sources().is_empty(), "offered once");

        rewrite(dir.path(), "script.cmt", "func update(dt: f32) { }");
        assert_eq!(runtime.changed_sources().len(), 1, "and the fix after it");
    }

    #[test]
    fn stopping_forgets_what_was_being_watched() {
        let (dir, mut scene, _node) = project("func update(dt: f32) { }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.play(&mut scene);
        runtime.stop(&mut scene);

        rewrite(dir.path(), "script.cmt", "func update(dt: f32) { }");
        assert!(
            runtime.changed_sources().is_empty(),
            "nothing is running, so nothing is watching"
        );
    }

    #[test]
    fn a_saved_script_swaps_in_under_the_running_game() {
        // The milestone's second half in one test: change the file, and what
        // the game does changes, without restarting it.
        let (dir, mut scene, node) =
            project("func update(dt: f32) { transform.position.x += 1.0; }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 1.0);

        rewrite(
            dir.path(),
            "script.cmt",
            "func update(dt: f32) { transform.position.x += 100.0; }",
        );
        for path in runtime.changed_sources() {
            runtime.reload(&mut scene, &path);
        }
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 101.0, "the new version is what ran");
        assert!(runtime.take_problems().is_empty());
    }

    #[test]
    fn a_reload_does_not_re_run_start() {
        // ADR 0008, amended when this was designed. A script whose `start`
        // places its node would teleport it back on every save, and the person
        // watching would conclude that saving resets the game.
        // `start` places the node, which is what `start` is for. If a reload
        // ran it, the sprite would jump back to 100 every time the file was
        // saved - and a save is not a request to restart the level.
        let source = "func start() { transform.position.x = 100.0; }
                      func update(dt: f32) { transform.position.x += 1.0; }";
        let (dir, mut scene, node) = project(source);

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        for _ in 0..5 {
            runtime.step(&mut scene, 0.016);
        }
        assert_eq!(x(&scene, node), 105.0);

        rewrite(dir.path(), "script.cmt", source);
        for path in runtime.changed_sources() {
            runtime.reload(&mut scene, &path);
        }
        runtime.step(&mut scene, 0.016);
        assert_eq!(
            x(&scene, node),
            106.0,
            "it carried on from where the game was, not from where it began"
        );
    }

    #[test]
    fn a_reload_keeps_the_value_the_component_holds() {
        // ADR 0022: the component owns an exported value, so a reload restores
        // what the inspector holds - not what the running script had made of
        // it. The alternative is a reload that quietly disagrees with the panel
        // next to it.
        let source = "@export let speed: f32 = 1.0;
                      func update(dt: f32) { speed = speed + 1.0; transform.position.x = speed; }";
        let (dir, mut scene, node) = project(source);
        let Some(Component::Script(script)) = scene.node_mut(node).components.get_mut(0) else {
            panic!("the fixture attaches a script");
        };
        script.exports = vec![("speed".to_string(), Value::F32(50.0))];

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        runtime.step(&mut scene, 0.016);
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 52.0, "the script has been adding to it");

        rewrite(dir.path(), "script.cmt", source);
        for path in runtime.changed_sources() {
            runtime.reload(&mut scene, &path);
        }
        runtime.step(&mut scene, 0.016);
        assert_eq!(
            x(&scene, node),
            51.0,
            "back to the tuned 50, not on from the module's 52"
        );
    }

    #[test]
    fn a_save_that_does_not_compile_leaves_the_game_running() {
        // Saving mid-thought is how people work. A game that stops dead on a
        // half-typed line is a game nobody can iterate on.
        let (dir, mut scene, node) =
            project("func update(dt: f32) { transform.position.x += 1.0; }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        runtime.step(&mut scene, 0.016);

        rewrite(dir.path(), "script.cmt", "func update(dt: f32) { $$$ }");
        for path in runtime.changed_sources() {
            runtime.reload(&mut scene, &path);
        }
        let problems = runtime.take_problems();
        assert_eq!(problems.len(), 1, "reported: {problems:?}");
        assert!(
            problems[0].to_string().contains("script.cmt"),
            "{problems:?}"
        );

        assert_eq!(runtime.len(), 1, "and still running");
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 2.0, "the last version that worked");

        // And a fix afterwards takes.
        rewrite(
            dir.path(),
            "script.cmt",
            "func update(dt: f32) { transform.position.x += 10.0; }",
        );
        for path in runtime.changed_sources() {
            runtime.reload(&mut scene, &path);
        }
        assert!(runtime.take_problems().is_empty());
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 12.0);
    }

    #[test]
    fn a_reload_swaps_every_node_running_that_file_and_keeps_the_order() {
        let (dir, mut scene, first) =
            project("func update(dt: f32) { transform.position.x += 1.0; }");
        let second = scene.add_child(scene.root(), Node::new("other"));
        attach_script(&mut scene, second, "script.cmt");

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, first, 0).expect("it compiles");
        runtime
            .attach(&mut scene, second, 0)
            .expect("so does the other");

        rewrite(
            dir.path(),
            "script.cmt",
            "func update(dt: f32) { transform.position.x += 5.0; }",
        );
        for path in runtime.changed_sources() {
            runtime.reload(&mut scene, &path);
        }
        assert_eq!(runtime.len(), 2, "replaced in place, not added to");
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, first), 5.0);
        assert_eq!(x(&scene, second), 5.0);
    }

    #[test]
    fn reloading_a_file_nothing_is_running_does_nothing() {
        let (dir, mut scene, node) = project("func update(dt: f32) { }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        runtime.reload(&mut scene, &dir.path().join("elsewhere.cmt"));
        assert_eq!(runtime.len(), 1);
        assert!(runtime.take_problems().is_empty());
    }

    #[test]
    fn a_frame_of_runaway_scripts_is_bounded() {
        // The per-call budget is 100ms and `step` had no clock of its own, so
        // twenty runaway scripts cost twenty times it - two seconds of a frozen
        // editor holding an unsaved scene.
        let (dir, mut scene, first) = project("func update(dt: f32) { while true { } }");
        for n in 0..19 {
            let node = scene.add_child(scene.root(), Node::new(format!("spin{n}")));
            attach_script(&mut scene, node, "script.cmt");
        }
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.play(&mut scene);
        assert_eq!(runtime.len(), 20);
        let _ = first;

        let started = std::time::Instant::now();
        runtime.step(&mut scene, 0.016);
        let took = started.elapsed();
        assert!(
            took < FRAME_LIMIT * 2,
            "the frame was bounded, took {took:?}"
        );
        // And it says what happened rather than leaving twenty silent skips.
        let problems = runtime.take_problems();
        assert!(
            problems
                .iter()
                .any(|p| p.to_string().contains("ran out of its")),
            "{problems:?}"
        );
    }

    #[test]
    fn one_runaway_still_leaves_the_others_their_frame() {
        // The guarantee the frame limit must not trade away. At exactly one
        // call budget a single runaway would eat the whole frame.
        let (dir, mut scene, spinner) = project("func update(dt: f32) { while true { } }");
        let good = scene.add_child(scene.root(), Node::new("good"));
        attach_script(&mut scene, good, "good.cmt");
        std::fs::write(
            dir.path().join("good.cmt"),
            "func update(dt: f32) { transform.position.x += 1.0; }",
        )
        .expect("writing the second script");

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, spinner, 0).expect("it compiles");
        runtime
            .attach(&mut scene, good, 0)
            .expect("so does the other");
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, good), 1.0);
    }

    #[test]
    fn a_script_broken_when_play_was_pressed_can_still_be_fixed() {
        // The beginner's loop: press Play, read the error, fix the typo, save.
        // The fix used to be invisible, because a file that did not compile was
        // never watched and `reload` scanned only what was running - so the one
        // script whose reload mattered most was the one that could not have one.
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(
            dir.path().join("script.cmt"),
            "func update(dt: f32) { $$$ }",
        )
        .expect("writing the broken script");
        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("player"));
        attach_script(&mut scene, node, "script.cmt");

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.play(&mut scene);
        assert!(runtime.is_empty(), "nothing compiled");
        assert_eq!(runtime.take_problems().len(), 1);

        rewrite(
            dir.path(),
            "script.cmt",
            "func update(dt: f32) { transform.position.x += 5.0; }",
        );
        let changed = runtime.changed_sources();
        assert_eq!(changed.len(), 1, "the fix was noticed");
        for path in changed {
            runtime.reload(&mut scene, &path);
        }
        assert_eq!(runtime.len(), 1, "and it is running now");
        runtime.step(&mut scene, 0.016);
        assert_eq!(x(&scene, node), 5.0);
    }

    #[test]
    fn a_session_can_say_what_is_running_and_what_has_stopped() {
        // The runtime knew all of this and none of it was reachable, so a
        // script that trapped was indistinguishable from one with nothing to
        // do - the game looked fine while one node sat still forever.
        let (dir, mut scene, spinner) = project("func update(dt: f32) { while true { } }");
        let good = scene.add_child(scene.root(), Node::new("good"));
        attach_script(&mut scene, good, "good.cmt");
        std::fs::write(
            dir.path().join("good.cmt"),
            "func update(dt: f32) { transform.position.x += 1.0; }",
        )
        .expect("writing the second script");

        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.play(&mut scene);
        assert!(
            runtime.instances().iter().all(|i| i.running),
            "everything starts running"
        );

        runtime.step(&mut scene, 0.016);
        let instances = runtime.instances();
        let stopped: Vec<&InstanceInfo> = instances.iter().filter(|i| !i.running).collect();
        assert_eq!(stopped.len(), 1, "{instances:?}");
        assert_eq!(stopped[0].node, spinner);
        assert_eq!(
            stopped[0].source, "script.cmt",
            "named as the component does"
        );
        assert_eq!(instances.len(), 2, "and the good one is still listed");
    }

    #[test]
    fn a_compile_failure_keeps_its_diagnostics_and_a_trap_keeps_its_function() {
        // helios goes to real trouble producing both, and flattening them into
        // a string here made Play's errors the only ones in the editor that
        // could not open a file or squiggle a line.
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(
            dir.path().join("script.cmt"),
            "func update(dt: f32) { $$$ }",
        )
        .expect("writing the broken script");
        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("player"));
        attach_script(&mut scene, node, "script.cmt");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.play(&mut scene);

        let problems = runtime.take_problems();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].source, "script.cmt");
        assert!(
            !problems[0].diagnostics.is_empty(),
            "the spans survived: {problems:?}"
        );
        // And it still reads as the line a shipped game would print.
        assert!(problems[0].to_string().starts_with("[script.cmt] "));

        // A trap keeps the comet function instead.
        let (dir, mut scene, node) =
            project("func update(dt: f32) { let a = [1.0]; let b = a[9]; }");
        let mut runtime = Runtime::new(dir.path()).expect("a runtime");
        runtime.attach(&mut scene, node, 0).expect("it compiles");
        runtime.step(&mut scene, 0.016);
        let problems = runtime.take_problems();
        assert_eq!(
            problems[0].function.as_deref(),
            Some("update"),
            "{problems:?}"
        );
    }
}
