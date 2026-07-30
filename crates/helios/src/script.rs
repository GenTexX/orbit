//! helios script host: compiling a node's Comet script and running it against
//! the real [`Scene`].
//!
//! This is the other half of the pipeline comet proved in isolation. comet
//! writes WebAssembly and never runs it; helios owns the [`Engine`], binds the
//! four host functions every comet module imports, and connects them to a live
//! [`Node`](crate::Node)'s [`Transform`](crate::Transform).
//!
//! # What a script can see
//!
//! Its own node's position, and nothing else. The store holds an owned copy,
//! handed in before each call and read back after, rather than borrowing the
//! `Scene`: v1's whole surface is `pos`, so there is nothing else to look at,
//! and a store that borrowed the scene would put its lifetime into every type
//! that touches a script. Copying in every frame also means an edit between
//! frames - dragging the node in the editor - is what the script sees next,
//! rather than the script fighting the editor from a stale position.
//!
//! `pos` is the node's **local** translation, the same value the inspector
//! shows. A script on a child moves it relative to its parent.
//!
//! # What this does not do
//!
//! Call anything per frame. Nothing here decides *when* a script runs - there
//! is one [`ScriptInstance::update`] and it runs one node for one frame.
//! Driving that from a game loop, and reloading a script when its file changes,
//! is milestone 5's job (ADR 0008).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::Vec2;
use thiserror::Error;
use wasmtime::{Caller, Engine, Extern, Linker, Module, Store, TypedFunc};

use crate::scene::{NodeId, Scene};

/// The function the host calls once per frame, if a script defines one.
const UPDATE: &str = "update";

/// How many printed lines an instance keeps before dropping the oldest. A script
/// printing in a loop must not grow memory without bound just because nobody
/// drained it this frame.
const MAX_PRINTED: usize = 512;

/// Something that went wrong compiling or running a script.
#[derive(Debug, Error)]
pub enum ScriptError {
    /// The script's file could not be read.
    #[error("reading script {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The script did not compile. Carries every diagnostic, spans included, so
    /// the editor can draw them rather than only report that something failed.
    #[error("{}", describe_diagnostics(.0))]
    Compile(Vec<comet::Diagnostic>),
    /// wasmtime refused the module, or the script trapped while running.
    #[error("{0}")]
    Runtime(String),
}

impl ScriptError {
    fn runtime(err: impl std::fmt::Display) -> Self {
        // `{:#}` keeps wasmtime's error chain, which is where the trap and its
        // backtrace live - a bare `{}` would report only the outermost sentence.
        ScriptError::Runtime(format!("{err:#}"))
    }
}

fn describe_diagnostics(diagnostics: &[comet::Diagnostic]) -> String {
    match diagnostics.first() {
        None => "the script did not compile".to_string(),
        Some(first) if diagnostics.len() == 1 => first.message.clone(),
        Some(first) => format!("{} (and {} more)", first.message, diagnostics.len() - 1),
    }
}

/// The engine and compiled-module cache shared by every script in a project.
///
/// One of these per editor or game: an [`Engine`] is expensive to build and
/// modules are only usable with the one that compiled them.
pub struct ScriptHost {
    engine: Engine,
    linker: Linker<ScriptState>,
    modules: HashMap<PathBuf, Module>,
}

impl ScriptHost {
    /// A host with the four comet imports bound.
    pub fn new() -> Result<Self, ScriptError> {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        bind_host(&mut linker)?;
        Ok(Self {
            engine,
            linker,
            modules: HashMap::new(),
        })
    }

    /// Compile `source` and start it for `node`, seeding the script from that
    /// node's current position.
    ///
    /// Takes the scene mutably because a state initializer can call a function,
    /// and that function can move the node - so instantiation is already a frame
    /// the scene has to hear about.
    pub fn instantiate(
        &mut self,
        source: &str,
        scene: &mut Scene,
        node: NodeId,
    ) -> Result<ScriptInstance, ScriptError> {
        let module = self.compile(source)?;
        self.start(&module, scene, node)
    }

    /// The same, reading the source from `path`. A path compiled before reuses
    /// its module: compiling is per source, starting is per node, and a scene
    /// with twenty nodes running one script should compile it once.
    pub fn instantiate_file(
        &mut self,
        path: &Path,
        scene: &mut Scene,
        node: NodeId,
    ) -> Result<ScriptInstance, ScriptError> {
        if !self.modules.contains_key(path) {
            let source = std::fs::read_to_string(path).map_err(|source| ScriptError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let module = self.compile(&source)?;
            self.modules.insert(path.to_path_buf(), module);
        }
        let module = self.modules[path].clone();
        self.start(&module, scene, node)
    }

    /// Forget the compiled module for `path`, so the next instantiation reads
    /// and compiles it again.
    pub fn forget(&mut self, path: &Path) {
        self.modules.remove(path);
    }

    fn compile(&self, source: &str) -> Result<Module, ScriptError> {
        let bytes = comet::compile(source).map_err(ScriptError::Compile)?;
        Module::new(&self.engine, &bytes).map_err(ScriptError::runtime)
    }

    fn start(
        &self,
        module: &Module,
        scene: &mut Scene,
        node: NodeId,
    ) -> Result<ScriptInstance, ScriptError> {
        let mut store = Store::new(
            &self.engine,
            ScriptState {
                position: scene.node(node).transform.translation,
                printed: Vec::new(),
            },
        );
        // Instantiation runs the module's start function, which is where a
        // script's state initializers evaluate.
        let instance = self
            .linker
            .instantiate(&mut store, module)
            .map_err(ScriptError::runtime)?;
        let update = instance.get_typed_func::<f32, ()>(&mut store, UPDATE).ok();
        scene.node_mut(node).transform.translation = store.data().position;
        Ok(ScriptInstance { store, update })
    }
}

/// One script running for one node: its own memory, globals, and state, which
/// outlive each call and are what makes a script's `let` at the top level
/// persistent.
pub struct ScriptInstance {
    store: Store<ScriptState>,
    update: Option<TypedFunc<f32, ()>>,
}

/// Neither a wasmtime `Store` nor a `TypedFunc` is `Debug`, so this reports what
/// a caller would actually want to see: whether there is a frame to run, and how
/// much output is waiting.
impl std::fmt::Debug for ScriptInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptInstance")
            .field("has_update", &self.update.is_some())
            .field("printed", &self.store.data().printed.len())
            .finish()
    }
}

impl ScriptInstance {
    /// Whether this script defines `update(dt: f32)`. One that does not is not
    /// broken - it may be a library of functions - so [`update`](Self::update)
    /// is a no-op rather than an error.
    pub fn has_update(&self) -> bool {
        self.update.is_some()
    }

    /// Run one frame for `node`: hand the script that node's position, call
    /// `update(dt)`, and write back wherever it moved to.
    pub fn update(&mut self, scene: &mut Scene, node: NodeId, dt: f32) -> Result<(), ScriptError> {
        let Some(update) = self.update.clone() else {
            return Ok(());
        };
        self.store.data_mut().position = scene.node(node).transform.translation;
        update
            .call(&mut self.store, dt)
            .map_err(ScriptError::runtime)?;
        scene.node_mut(node).transform.translation = self.store.data().position;
        Ok(())
    }

    /// Take the lines this script printed since the last call, leaving it empty.
    pub fn take_printed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.store.data_mut().printed)
    }
}

/// What a running script can see and touch.
#[derive(Debug)]
struct ScriptState {
    position: Vec2,
    printed: Vec<String>,
}

/// Bind the four functions every comet module imports (see `comet::HOST_MODULE`
/// - the import list is fixed, so one binding table serves every script).
fn bind_host(linker: &mut Linker<ScriptState>) -> Result<(), ScriptError> {
    let host = comet::HOST_MODULE;
    linker
        .func_wrap(host, "get_position_x", |c: Caller<'_, ScriptState>| {
            c.data().position.x
        })
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(host, "get_position_y", |c: Caller<'_, ScriptState>| {
            c.data().position.y
        })
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "set_position",
            |mut c: Caller<'_, ScriptState>, x: f32, y: f32| {
                c.data_mut().position = Vec2::new(x, y);
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "print",
            |mut c: Caller<'_, ScriptState>, ptr: i32, len: i32| {
                let text = read_string(&mut c, ptr, len);
                let printed = &mut c.data_mut().printed;
                if printed.len() >= MAX_PRINTED {
                    printed.remove(0);
                }
                printed.push(text);
            },
        )
        .map_err(ScriptError::runtime)?;
    Ok(())
}

/// Read `len` bytes at `ptr` out of the module's own exported memory. A script
/// cannot hand the host anything but an offset into its own linear memory, so a
/// bad one is clamped to nothing rather than trusted into a panic.
fn read_string(caller: &mut Caller<'_, ScriptState>, ptr: i32, len: i32) -> String {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return String::new();
    };
    let data = memory.data(&caller);
    let start = ptr.max(0) as usize;
    let end = start.saturating_add(len.max(0) as usize).min(data.len());
    if start >= end {
        return String::new();
    }
    String::from_utf8_lossy(&data[start..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{Component, ScriptComponent};
    use crate::scene::Node;
    use crate::transform::Transform;

    /// The roadmap's example: a node that walks along X and turns around at the
    /// edges, with its speed and direction kept in script state.
    const BOUNCE: &str = "
        let speed = 120.0;
        let direction = 1.0;

        func update(dt: f32) {
            pos.x += speed * direction * dt;
            if pos.x > 200.0 { direction = -1.0; }
            if pos.x < -200.0 { direction = 1.0; }
        }
    ";

    /// A scene with one node at `at`, and its handle.
    fn scene_with_node(at: Vec2) -> (Scene, NodeId) {
        let mut scene = Scene::new("root");
        let root = scene.root();
        let mut node = Node::new("player");
        node.transform = Transform::from_translation(at);
        node.components.push(Component::Script(ScriptComponent {
            source: "scripts/bounce.cmt".into(),
        }));
        let node = scene.add_child(root, node);
        (scene, node)
    }

    fn position(scene: &Scene, node: NodeId) -> Vec2 {
        scene.node(node).transform.translation
    }

    // --- the milestone's proof point ---

    #[test]
    fn a_script_moves_its_node_in_a_real_scene() {
        // The roadmap's literal ask, end to end: source text in, a real Node's
        // Transform changed out, with nothing faked in between - comet compiled
        // it, wasmtime ran it, and the position came back through the host.
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate(BOUNCE, &mut scene, node)
            .expect("bounce compiles");

        script.update(&mut scene, node, 0.5).expect("one frame");
        assert_eq!(
            position(&scene, node),
            Vec2::new(60.0, 0.0),
            "120 * 1 * 0.5"
        );

        script.update(&mut scene, node, 0.5).expect("another frame");
        assert_eq!(
            position(&scene, node),
            Vec2::new(120.0, 0.0),
            "state persisted, so it kept going the same way"
        );
    }

    #[test]
    fn a_script_turns_around_where_it_says_it_does() {
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host.instantiate(BOUNCE, &mut scene, node).unwrap();

        for _ in 0..4 {
            script.update(&mut scene, node, 0.5).unwrap();
        }
        assert_eq!(position(&scene, node).x, 240.0, "past the far edge");
        script.update(&mut scene, node, 0.5).unwrap();
        assert_eq!(position(&scene, node).x, 180.0, "and back the other way");
    }

    #[test]
    fn a_script_moves_only_the_node_it_runs_for() {
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let root = scene.root();
        let other = scene.add_child(root, Node::new("bystander"));

        let mut script = host.instantiate(BOUNCE, &mut scene, node).unwrap();
        script.update(&mut scene, node, 0.5).unwrap();

        assert_eq!(position(&scene, node), Vec2::new(60.0, 0.0));
        assert_eq!(position(&scene, other), Vec2::ZERO, "untouched");
    }

    #[test]
    fn writing_one_axis_leaves_the_other_where_the_scene_had_it() {
        // The script only ever assigns pos.x, so y must survive the round trip
        // out to the script and back.
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::new(0.0, 42.0));
        let mut script = host.instantiate(BOUNCE, &mut scene, node).unwrap();
        script.update(&mut scene, node, 0.5).unwrap();
        assert_eq!(position(&scene, node), Vec2::new(60.0, 42.0));
    }

    // --- the editor and the script share one position ---

    #[test]
    fn a_node_moved_between_frames_is_where_the_script_carries_on_from() {
        // Dragging a node in the editor must not be undone by the script's idea
        // of where it was last frame. The position goes in fresh every call, so
        // the script continues from where the scene actually is.
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host.instantiate(BOUNCE, &mut scene, node).unwrap();

        script.update(&mut scene, node, 0.5).unwrap();
        assert_eq!(position(&scene, node).x, 60.0);

        // The editor drags it somewhere else entirely.
        scene.node_mut(node).transform.translation = Vec2::new(-500.0, 0.0);
        script.update(&mut scene, node, 0.5).unwrap();
        assert_eq!(
            position(&scene, node).x,
            -440.0,
            "it stepped on from the new position, not from 60"
        );
    }

    #[test]
    fn a_script_starts_from_the_position_the_node_already_has() {
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::new(10.0, 20.0));
        let mut script = host
            .instantiate(
                "
                let home = pos;
                func update(dt: f32) { pos = home; }
                ",
                &mut scene,
                node,
            )
            .unwrap();

        scene.node_mut(node).transform.translation = Vec2::new(999.0, 999.0);
        script.update(&mut scene, node, 0.0).unwrap();
        assert_eq!(
            position(&scene, node),
            Vec2::new(10.0, 20.0),
            "state captured the position at instantiation"
        );
    }

    #[test]
    fn a_state_initializer_that_moves_the_node_is_written_back() {
        // A top-level `let` cannot assign, but it can call a function that does,
        // so instantiation is already a frame the scene has to hear about.
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        host.instantiate(
            "
            func jump() -> f32 {
                pos.y = 77.0;
                1.0
            }
            let started = jump();
            ",
            &mut scene,
            node,
        )
        .unwrap();
        assert_eq!(position(&scene, node), Vec2::new(0.0, 77.0));
    }

    // --- print ---

    #[test]
    fn print_reaches_the_host_as_text() {
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate(
                r#"
                let ticks = 0.0;
                func update(dt: f32) {
                    ticks += dt;
                    if ticks > 1.0 {
                        print("one second passed");
                        ticks = 0.0;
                    }
                }
                "#,
                &mut scene,
                node,
            )
            .unwrap();

        script.update(&mut scene, node, 0.6).unwrap();
        assert!(script.take_printed().is_empty(), "not a second yet");
        script.update(&mut scene, node, 0.6).unwrap();
        assert_eq!(script.take_printed(), ["one second passed"]);
        assert!(script.take_printed().is_empty(), "taking empties it");
    }

    // --- failure, reported rather than thrown ---

    #[test]
    fn a_script_that_does_not_compile_hands_back_its_diagnostics() {
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let err = host
            .instantiate(
                "func update(dt: f32) { let ready = true; pos.x += ready; }",
                &mut scene,
                node,
            )
            .expect_err("a type error must not start");

        let ScriptError::Compile(diagnostics) = &err else {
            panic!("expected a compile error, got {err:?}");
        };
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "expected `f32`, found `bool`");
        // The spans travel with it, which is what the editor draws squiggles
        // from - reporting only "it failed" would be useless there.
        assert!(diagnostics[0].span.end > diagnostics[0].span.start);
        assert!(err.to_string().contains("expected `f32`"));
    }

    #[test]
    fn a_script_with_no_update_runs_no_frames_and_says_so() {
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::new(5.0, 5.0));
        let mut script = host
            .instantiate("func helper(a: f32) -> f32 { a * 2.0 }", &mut scene, node)
            .unwrap();
        assert!(!script.has_update());
        script.update(&mut scene, node, 1.0).expect("a no-op");
        assert_eq!(position(&scene, node), Vec2::new(5.0, 5.0));
    }

    #[test]
    fn a_missing_script_file_is_an_io_error_naming_the_path() {
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let err = host
            .instantiate_file(Path::new("/nowhere/missing.cmt"), &mut scene, node)
            .expect_err("no such file");
        assert!(matches!(err, ScriptError::Io { .. }));
        assert!(err.to_string().contains("missing.cmt"));
    }

    // --- compiling once, running many ---

    #[test]
    fn one_file_compiles_once_and_starts_independently_per_node() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bounce.cmt");
        std::fs::write(&path, BOUNCE).unwrap();

        let mut host = ScriptHost::new().unwrap();
        let mut scene = Scene::new("root");
        let root = scene.root();
        let a = scene.add_child(root, Node::new("a"));
        let b = scene.add_child(root, Node::new("b"));

        let mut script_a = host.instantiate_file(&path, &mut scene, a).unwrap();
        let mut script_b = host.instantiate_file(&path, &mut scene, b).unwrap();

        // Deleting the file after the first read proves the second instance came
        // from the cache rather than reading it again.
        std::fs::remove_file(&path).unwrap();
        let mut script_c = host
            .instantiate_file(&path, &mut scene, a)
            .expect("the module was cached");

        // Each instance keeps its own state: stepping one does not move another.
        script_a.update(&mut scene, a, 0.5).unwrap();
        script_a.update(&mut scene, a, 0.5).unwrap();
        script_b.update(&mut scene, b, 0.5).unwrap();
        assert_eq!(position(&scene, a).x, 120.0);
        assert_eq!(position(&scene, b).x, 60.0, "b ran once, not three times");

        // And forgetting the path means the next one has to read it again.
        script_c.update(&mut scene, a, 0.0).unwrap();
        host.forget(&path);
        assert!(matches!(
            host.instantiate_file(&path, &mut scene, a),
            Err(ScriptError::Io { .. })
        ));
    }
}
