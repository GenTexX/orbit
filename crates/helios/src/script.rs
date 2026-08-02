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
//! `Scene`: v1's whole surface is the node's own transform, so there is
//! nothing else to look at,
//! and a store that borrowed the scene would put its lifetime into every type
//! that touches a script. Copying in every frame also means an edit between
//! frames - dragging the node in the editor - is what the script sees next,
//! rather than the script fighting the editor from a stale position.
//!
//! `transform.position` is the node's **local** translation, the same value the inspector
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
use wasmtime::{Caller, Engine, Extern, Instance, Linker, Module, Store, TypedFunc, Val};

use crate::reflect::Value;
use crate::scene::{NodeId, Scene};
use crate::transform::Transform;

/// The function the host calls once per frame, if a script defines one.
const UPDATE: &str = "update";
/// Run once before the first `update`, after state initializers. It is also
/// what stops a top-level `let` doing double duty as init code.
const START: &str = "start";
/// Run when the node or the script goes away. Cheap to add now and awkward to
/// retrofit once instances have real lifetimes (M5).
const ON_DESTROY: &str = "on_destroy";

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
    /// The script trapped while running, in a function comet can name.
    ///
    /// Separate from [`Runtime`](Self::Runtime) because an editor can act on
    /// this one: it has somewhere to put the caret.
    #[error("{message} (in `{function}`)")]
    Trapped { function: String, message: String },
}

impl ScriptError {
    /// The script function a trap happened in, if the error is one and the
    /// module's name section named it.
    pub fn trapped_in(&self) -> Option<&str> {
        match self {
            ScriptError::Trapped { function, .. } => Some(function),
            _ => None,
        }
    }

    /// Turn a wasmtime failure into an error that names the script function it
    /// happened in, where the backtrace has one.
    ///
    /// comet emits a name section, so the frames carry real names rather than
    /// `wasm-function[7]`. The innermost script frame is the useful one: the
    /// runtime helpers below it (`comet_alloc` and friends) are true but not
    /// where anybody's mistake is.
    fn trap(err: wasmtime::Error) -> Self {
        let function = err
            .downcast_ref::<wasmtime::WasmBacktrace>()
            .and_then(|backtrace| {
                backtrace
                    .frames()
                    .iter()
                    .filter_map(|frame| frame.func_name())
                    .find(|name| !name.starts_with("comet_") && !name.starts_with("orbit::"))
                    .map(str::to_string)
            });
        match function {
            Some(function) => ScriptError::Trapped {
                function,
                message: format!("{err:#}"),
            },
            None => ScriptError::runtime(err),
        }
    }

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
        self.instantiate_with(source, scene, node, &[])
    }

    /// The same, with the component's stored values for the script's
    /// `@export`ed variables. Those are written in after instantiation and
    /// before `start`, because the inspector owns them and the initializer in
    /// the source is only the default (ADR 0022).
    pub fn instantiate_with(
        &mut self,
        source: &str,
        scene: &mut Scene,
        node: NodeId,
        exports: &[(String, Value)],
    ) -> Result<ScriptInstance, ScriptError> {
        let module = self.compile(source)?;
        self.start(&module, scene, node, exports)
    }

    /// The same, reading the source from `path`. A path compiled before reuses
    /// its module: compiling is per source, starting is per node, and a scene
    /// with twenty nodes running one script should compile it once.
    pub fn instantiate_file(
        &mut self,
        path: &Path,
        scene: &mut Scene,
        node: NodeId,
        exports: &[(String, Value)],
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
        let mut instance = self.start(&module, scene, node, exports)?;
        instance.set_label(path.display().to_string());
        Ok(instance)
    }

    /// Forget the compiled module for `path`, so the next instantiation reads
    /// and compiles it again.
    pub fn forget(&mut self, path: &Path) {
        self.modules.remove(path);
    }

    fn compile(&self, source: &str) -> Result<Module, ScriptError> {
        let bytes = comet::compile(source, schema()).map_err(ScriptError::Compile)?;
        Module::new(&self.engine, &bytes).map_err(ScriptError::runtime)
    }

    fn start(
        &self,
        module: &Module,
        scene: &mut Scene,
        node: NodeId,
        exports: &[(String, Value)],
    ) -> Result<ScriptInstance, ScriptError> {
        let mut store = Store::new(
            &self.engine,
            ScriptState {
                transform: scene.node(node).transform,
                printed: Vec::new(),
            },
        );
        // Instantiation runs the module's start function, which is where a
        // script's state initializers evaluate.
        let instance = self
            .linker
            .instantiate(&mut store, module)
            .map_err(ScriptError::runtime)?;
        // The inspector's stored values go in before anything runs. The
        // script's own initializer has already evaluated during instantiation
        // and is only the default; the stored value is what wins (ADR 0022),
        // and `start` below must see it.
        apply_exports(&mut store, &instance, exports);
        let update = instance.get_typed_func::<f32, ()>(&mut store, UPDATE).ok();
        let on_destroy = instance
            .get_typed_func::<(), ()>(&mut store, ON_DESTROY)
            .ok();
        // Before the write-back, so a `start` that moves the node is seen.
        if let Ok(start) = instance.get_typed_func::<(), ()>(&mut store, START) {
            start.call(&mut store, ()).map_err(ScriptError::trap)?;
        }
        scene.node_mut(node).transform = store.data().transform;
        Ok(ScriptInstance {
            store,
            update,
            on_destroy,
            label: scene.node(node).name.clone(),
        })
    }
}

/// One script running for one node: its own memory, globals, and state, which
/// outlive each call and are what makes a script's `let` at the top level
/// persistent.
pub struct ScriptInstance {
    store: Store<ScriptState>,
    update: Option<TypedFunc<f32, ()>>,
    on_destroy: Option<TypedFunc<(), ()>>,
    /// What this instance's printed output is tagged with.
    label: String,
}

/// Neither a wasmtime `Store` nor a `TypedFunc` is `Debug`, so this reports what
/// a caller would actually want to see: whether there is a frame to run, and how
/// much output is waiting.
impl std::fmt::Debug for ScriptInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptInstance")
            .field("label", &self.label)
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
        self.store.data_mut().transform = scene.node(node).transform;
        update
            .call(&mut self.store, dt)
            .map_err(ScriptError::trap)?;
        scene.node_mut(node).transform = self.store.data().transform;
        Ok(())
    }

    /// Run `on_destroy()`, if the script defines one, because the node or the
    /// script is going away.
    ///
    /// Called by whoever owns the instance rather than from `Drop`: the hook can
    /// trap, and a `Drop` has nowhere to report that to. Nothing calls this on a
    /// real lifecycle yet - instances do not have one until Play exists - which
    /// is exactly why the mechanism goes in now rather than being retrofitted
    /// around it later.
    pub fn destroy(&mut self, scene: &mut Scene, node: NodeId) -> Result<(), ScriptError> {
        let Some(on_destroy) = self.on_destroy.take() else {
            return Ok(());
        };
        self.store.data_mut().transform = scene.node(node).transform;
        on_destroy
            .call(&mut self.store, ())
            .map_err(ScriptError::trap)?;
        scene.node_mut(node).transform = self.store.data().transform;
        Ok(())
    }

    /// Take the lines this script printed since the last call, leaving it empty.
    pub fn take_printed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.store.data_mut().printed)
    }

    /// The same, each line prefixed with where it came from - what a console
    /// showing several scripts at once needs, since `print("hi")` from two
    /// nodes is otherwise two identical lines.
    pub fn take_printed_tagged(&mut self) -> Vec<String> {
        let label = self.label.clone();
        self.take_printed()
            .into_iter()
            .map(|line| format!("[{label}] {line}"))
            .collect()
    }

    /// What this instance's output is tagged with - its script's path, or the
    /// node it runs for when it was compiled from source.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Rename the instance, for a host that knows better than the default.
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
    }
}

/// What a running script can see and touch.
#[derive(Debug)]
struct ScriptState {
    transform: Transform,
    printed: Vec<String>,
}

/// Write the component's stored values into the module's exported globals.
///
/// A value that does not line up - the script no longer exports that name, or
/// exports it at another type - is skipped rather than forced. The component is
/// reconciled against the script separately; this is the last line of defence
/// for a module and a component that disagree, and dropping a value is better
/// than writing a float into a bool.
fn apply_exports(store: &mut Store<ScriptState>, instance: &Instance, exports: &[(String, Value)]) {
    for (name, value) in exports {
        let globals = match value {
            Value::Vec2(_) => comet::exported_globals(name, comet::Type::Vec2),
            Value::F32(_) => comet::exported_globals(name, comet::Type::F32),
            Value::Bool(_) => comet::exported_globals(name, comet::Type::Bool),
            _ => continue,
        };
        let wasm: Vec<Val> = match value {
            Value::F32(v) => vec![Val::F32(v.to_bits())],
            Value::Bool(v) => vec![Val::I32(i32::from(*v))],
            Value::Vec2(v) => vec![Val::F32(v.x.to_bits()), Val::F32(v.y.to_bits())],
            _ => continue,
        };
        for (name, value) in globals.iter().zip(wasm) {
            if let Some(global) = instance.get_global(&mut *store, name) {
                let _ = global.set(&mut *store, value);
            }
        }
    }
}

/// Which property a schema id refers to. The order here is what fixes the ids,
/// and [`schema`] is built from the same list, so the two cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Property {
    Position,
    Rotation,
    Scale,
}

/// Everything a script can reach outside itself, and what each thing is.
///
/// comet compiles against the schema this produces and never learns what a
/// `Transform` is. Adding a property here makes it scriptable - with
/// completions and hover - without touching the compiler.
const PROPERTIES: &[(&str, &str, comet::HostType, Property)] = &[
    (
        "transform",
        "position",
        comet::HostType::Vec2,
        Property::Position,
    ),
    (
        "transform",
        "rotation",
        comet::HostType::F32,
        Property::Rotation,
    ),
    ("transform", "scale", comet::HostType::Vec2, Property::Scale),
];

/// The host surface every script in this engine is compiled against.
///
/// The editor asks for this too, so its squiggles and completions describe the
/// same surface the compiler enforces - one definition, not two.
pub fn schema() -> &'static comet::HostSchema {
    static SCHEMA: std::sync::OnceLock<comet::HostSchema> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(build_schema)
}

fn build_schema() -> comet::HostSchema {
    let mut schema = comet::HostSchema::new();
    let mut at = 0;
    while at < PROPERTIES.len() {
        let object = PROPERTIES[at].0;
        let fields: Vec<(&'static str, comet::HostType)> = PROPERTIES[at..]
            .iter()
            .take_while(|(o, ..)| *o == object)
            .map(|(_, field, ty, _)| (*field, *ty))
            .collect();
        at += fields.len();
        schema = schema.object(object, fields);
    }
    schema
}

/// What property `id` names, or `None` if a module asked for one that is not in
/// the schema it was compiled against - which can only happen if a module and
/// the engine disagree, so it reads as a default rather than a panic.
fn property(id: i32) -> Option<Property> {
    usize::try_from(id)
        .ok()
        .and_then(|id| PROPERTIES.get(id))
        .map(|(.., property)| *property)
}

/// Bind the functions every comet module imports (see `comet::HOST_MODULE`
/// - the import list is fixed, so one binding table serves every script).
fn bind_host(linker: &mut Linker<ScriptState>) -> Result<(), ScriptError> {
    let host = comet::HOST_MODULE;
    // The property accessors. Each takes the property's schema id, so this
    // table stays the same size however many properties the engine exposes.
    linker
        .func_wrap(
            host,
            "get_f32",
            |c: Caller<'_, ScriptState>, id: i32| match property(id) {
                Some(Property::Rotation) => c.data().transform.rotation,
                _ => 0.0,
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "set_f32",
            |mut c: Caller<'_, ScriptState>, id: i32, value: f32| {
                if let Some(Property::Rotation) = property(id) {
                    c.data_mut().transform.rotation = value;
                }
            },
        )
        .map_err(ScriptError::runtime)?;
    // No property is a bool yet. The accessors exist so that adding one is a
    // change to PROPERTIES rather than to the ABI.
    linker
        .func_wrap(host, "get_bool", |_: Caller<'_, ScriptState>, _id: i32| {
            0i32
        })
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "set_bool",
            |_: Caller<'_, ScriptState>, _id: i32, _value: i32| {},
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "get_vec2_x",
            |c: Caller<'_, ScriptState>, id: i32| match property(id) {
                Some(Property::Position) => c.data().transform.translation.x,
                Some(Property::Scale) => c.data().transform.scale.x,
                _ => 0.0,
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "get_vec2_y",
            |c: Caller<'_, ScriptState>, id: i32| match property(id) {
                Some(Property::Position) => c.data().transform.translation.y,
                Some(Property::Scale) => c.data().transform.scale.y,
                _ => 0.0,
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "set_vec2",
            |mut c: Caller<'_, ScriptState>, id: i32, x: f32, y: f32| {
                let value = Vec2::new(x, y);
                match property(id) {
                    Some(Property::Position) => c.data_mut().transform.translation = value,
                    Some(Property::Scale) => c.data_mut().transform.scale = value,
                    _ => {}
                }
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "str",
            |mut c: Caller<'_, ScriptState>, value: f32| -> Result<i32, wasmtime::Error> {
                // The one host function that allocates. It calls back into the
                // module rather than reserving memory of its own, so what it
                // returns is an ordinary refcounted block the script releases
                // like any other String.
                let text = comet::format_f32(value);
                let Some(Extern::Func(alloc)) = c.get_export("comet_alloc") else {
                    return Err(wasmtime::Error::msg(
                        "every comet module exports comet_alloc",
                    ));
                };
                let alloc = alloc.typed::<i32, i32>(&c)?;
                let ptr = alloc.call(&mut c, text.len() as i32)?;
                let Some(Extern::Memory(memory)) = c.get_export("memory") else {
                    return Err(wasmtime::Error::msg(
                        "every comet module exports its memory",
                    ));
                };
                comet::write_str(memory.data_mut(&mut c), ptr, &text);
                Ok(ptr)
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "str_int",
            |mut c: Caller<'_, ScriptState>, value: i32| -> Result<i32, wasmtime::Error> {
                // Whole numbers print whole. Widening to f32 first would round
                // past 2^24, silently, in the one function used to see a value.
                let text = value.to_string();
                let Some(Extern::Func(alloc)) = c.get_export("comet_alloc") else {
                    return Err(wasmtime::Error::msg(
                        "every comet module exports comet_alloc",
                    ));
                };
                let alloc = alloc.typed::<i32, i32>(&c)?;
                let ptr = alloc.call(&mut c, text.len() as i32)?;
                let Some(Extern::Memory(memory)) = c.get_export("memory") else {
                    return Err(wasmtime::Error::msg(
                        "every comet module exports its memory",
                    ));
                };
                comet::write_str(memory.data_mut(&mut c), ptr, &text);
                Ok(ptr)
            },
        )
        .map_err(ScriptError::runtime)?;
    // The transcendentals. Everything else comet needs from maths is one
    // WebAssembly instruction and never leaves the module.
    linker
        .func_wrap(host, "sin", |_: Caller<'_, ScriptState>, x: f32| x.sin())
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(host, "cos", |_: Caller<'_, ScriptState>, x: f32| x.cos())
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "atan2",
            |_: Caller<'_, ScriptState>, y: f32, x: f32| y.atan2(x),
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(host, "pow", |_: Caller<'_, ScriptState>, a: f32, b: f32| {
            a.powf(b)
        })
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
    use crate::reflect::Reflect;
    use crate::scene::Node;
    use crate::transform::Transform;

    /// The roadmap's example: a node that walks along X and turns around at the
    /// edges, with its speed and direction kept in script state.
    const BOUNCE: &str = "
        let speed = 120.0;
        let direction = 1.0;

        func update(dt: f32) {
            transform.position.x += speed * direction * dt;
            if transform.position.x > 200.0 { direction = -1.0; }
            if transform.position.x < -200.0 { direction = 1.0; }
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
            ..Default::default()
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
        // The script only ever assigns transform.position.x, so y must survive the round trip
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
                let home = transform.position;
                func update(dt: f32) { transform.position = home; }
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
                transform.position.y = 77.0;
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
                "func update(dt: f32) { let ready = true; transform.position.x += ready; }",
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
    fn a_trap_names_the_script_function_it_happened_in() {
        // A wasm trap otherwise reports `wasm-function[7]`. comet emits a name
        // section so the frames carry real names, and the runtime helpers are
        // skipped - true, but not where anybody's mistake is.
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        // A script with a type error cannot compile, so reach for the one thing
        // that traps at runtime: an allocation far larger than memory can grow.
        // Unbounded recursion is the trap a person actually writes by accident.
        let mut script = host
            .instantiate(
                "
                func forever(n: f32) -> f32 { forever(n) }
                func update(dt: f32) { transform.position.x = forever(dt); }
                ",
                &mut scene,
                node,
            )
            .unwrap();
        let err = script
            .update(&mut scene, node, 1.0)
            .expect_err("recursing forever must trap");
        assert_eq!(
            err.trapped_in(),
            Some("forever"),
            "named, not wasm-function[n]: {err}"
        );
        assert!(err.to_string().contains("forever"));

        // And comet can turn that name into a line to put the caret on.
        assert_eq!(
            comet::service::function_line(
                "\nfunc forever(n: f32) -> f32 { forever(n) }\n",
                "forever"
            ),
            Some(2)
        );
    }

    #[test]
    fn printed_output_can_say_which_script_it_came_from() {
        // Two nodes printing the same words are otherwise two identical lines.
        let mut host = ScriptHost::new().unwrap();
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate(
                r#"func update(dt: f32) { print("tick"); }"#,
                &mut scene,
                node,
            )
            .unwrap();
        script.update(&mut scene, node, 0.1).unwrap();
        assert_eq!(script.take_printed_tagged(), ["[player] tick"]);

        script.set_label("scripts/ticker.cmt");
        script.update(&mut scene, node, 0.1).unwrap();
        assert_eq!(script.take_printed_tagged(), ["[scripts/ticker.cmt] tick"]);
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
            .instantiate_file(Path::new("/nowhere/missing.cmt"), &mut scene, node, &[])
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

        let mut script_a = host.instantiate_file(&path, &mut scene, a, &[]).unwrap();
        let mut script_b = host.instantiate_file(&path, &mut scene, b, &[]).unwrap();

        // Deleting the file after the first read proves the second instance came
        // from the cache rather than reading it again.
        std::fs::remove_file(&path).unwrap();
        let mut script_c = host
            .instantiate_file(&path, &mut scene, a, &[])
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
            host.instantiate_file(&path, &mut scene, a, &[]),
            Err(ScriptError::Io { .. })
        ));
    }

    #[test]
    fn start_runs_once_before_the_first_update() {
        // What `start` is for: a top-level `let` was doing double duty as init
        // code, and the demo script needed a comment to explain it.
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::new(5.0, 5.0));
        let mut script = host
            .instantiate(
                "
                let ticks = 0.0;
                func start() {
                    transform.position = vec2(100.0, 200.0);
                }
                func update(dt: f32) {
                    ticks += 1.0;
                }
                ",
                &mut scene,
                node,
            )
            .expect("it compiles");

        // Before any update at all.
        assert_eq!(position(&scene, node), Vec2::new(100.0, 200.0));

        script.update(&mut scene, node, 0.5).expect("one frame");
        script.update(&mut scene, node, 0.5).expect("another");
        assert_eq!(
            position(&scene, node),
            Vec2::new(100.0, 200.0),
            "start does not run again"
        );
    }

    #[test]
    fn start_sees_the_state_its_initializers_produced() {
        // Instantiation runs the module's own start function, where state
        // initializers evaluate, so the hook must run after that and not
        // instead of it.
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        host.instantiate(
            "
            let home: Vec2 = vec2(7.0, 9.0);
            func start() { transform.position = home; }
            func update(dt: f32) { }
            ",
            &mut scene,
            node,
        )
        .expect("it compiles");
        assert_eq!(position(&scene, node), Vec2::new(7.0, 9.0));
    }

    #[test]
    fn on_destroy_runs_when_the_owner_says_the_script_is_going_away() {
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate(
                r#"
                func update(dt: f32) { }
                func on_destroy() { print("goodbye"); }
                "#,
                &mut scene,
                node,
            )
            .expect("it compiles");

        assert!(
            script.take_printed().is_empty(),
            "not until it is destroyed"
        );
        script.destroy(&mut scene, node).expect("the hook runs");
        assert_eq!(script.take_printed(), ["goodbye"]);

        // Destroying twice does not run it twice.
        script.destroy(&mut scene, node).expect("no hook left");
        assert!(script.take_printed().is_empty());
    }

    #[test]
    fn a_script_with_neither_hook_is_not_broken() {
        // Both are optional. A script that is a library of functions defines
        // none of them and must still instantiate and tick.
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate("func helper(a: f32) -> f32 { a }", &mut scene, node)
            .expect("it compiles");
        script.update(&mut scene, node, 0.5).expect("a no-op frame");
        script.destroy(&mut scene, node).expect("a no-op destroy");
    }

    #[test]
    fn a_script_rotates_and_scales_its_node_through_the_schema() {
        // Nothing but `pos` could be reached before. These two properties cost
        // a row each in PROPERTIES - no new IR, no new import, no compiler
        // change - which is the whole claim decision 2 makes.
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::new(5.0, 5.0));
        let mut script = host
            .instantiate(
                "func update(dt: f32) {
                     transform.rotation = 1.5;
                     transform.scale = vec2(2.0, 4.0);
                 }",
                &mut scene,
                node,
            )
            .expect("it compiles");
        script.update(&mut scene, node, 0.0).expect("one frame");

        let transform = scene.node(node).transform;
        assert_eq!(transform.rotation, 1.5);
        assert_eq!(transform.scale, Vec2::new(2.0, 4.0));
        assert_eq!(
            transform.translation,
            Vec2::new(5.0, 5.0),
            "the property it did not touch is where the scene had it"
        );
    }

    #[test]
    fn the_schema_and_the_property_table_cannot_disagree() {
        // The ids codegen emits are positions in PROPERTIES, and the schema is
        // built from the same list. If the two ever drift, every accessor reads
        // the wrong field - so this checks they are still derived from one
        // another rather than maintained in parallel.
        let schema = schema();
        for (id, (object, field, ty, _)) in PROPERTIES.iter().enumerate() {
            let found = schema
                .resolve(object, field)
                .unwrap_or_else(|| panic!("{object}.{field} is missing from the schema"));
            assert_eq!(found.id as usize, id, "{object}.{field} is numbered wrong");
            assert_eq!(found.ty, *ty);
            assert_eq!(property(id as i32), Some(PROPERTIES[id].3));
        }
        assert_eq!(schema.properties().count(), PROPERTIES.len());
        assert_eq!(property(PROPERTIES.len() as i32), None, "and no further");
    }

    #[test]
    fn a_script_reaching_for_something_the_engine_does_not_expose_is_refused() {
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let error = host
            .instantiate(
                "func update(dt: f32) { transform.mass = 1.0; }",
                &mut scene,
                node,
            )
            .expect_err("there is no such property");
        let ScriptError::Compile(diagnostics) = error else {
            panic!("expected a compile error, got {error:?}");
        };
        assert!(
            diagnostics[0].message.contains("has no property `mass`"),
            "{:?}",
            diagnostics[0].message
        );
    }

    // --- exported variables ---

    const TUNABLE: &str = "
        @export let speed: f32;
        func update(dt: f32) {
            transform.position.x += speed * dt;
        }
    ";

    #[test]
    fn two_nodes_run_one_script_at_different_speeds() {
        // The whole point of decision 12, and the thing M5's hot-reload promise
        // is measured against: without this, a script has no per-instance
        // state for reloading to preserve.
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, fast) = scene_with_node(Vec2::ZERO);
        let slow = scene.add_child(scene.root(), Node::new("slow"));

        let mut a = host
            .instantiate_with(
                TUNABLE,
                &mut scene,
                fast,
                &[("speed".into(), Value::F32(100.0))],
            )
            .expect("it compiles");
        let mut b = host
            .instantiate_with(
                TUNABLE,
                &mut scene,
                slow,
                &[("speed".into(), Value::F32(10.0))],
            )
            .expect("it compiles");

        a.update(&mut scene, fast, 1.0).expect("one frame");
        b.update(&mut scene, slow, 1.0).expect("one frame");
        assert_eq!(position(&scene, fast).x, 100.0);
        assert_eq!(position(&scene, slow).x, 10.0);
    }

    #[test]
    fn the_stored_value_beats_the_one_written_in_the_script() {
        // The initializer is the default used when the field is first created
        // or explicitly reverted. Anything stored wins.
        let source = "
            @export let speed: f32 = 999.0;
            func update(dt: f32) { transform.position.x += speed * dt; }
        ";
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate_with(
                source,
                &mut scene,
                node,
                &[("speed".into(), Value::F32(2.0))],
            )
            .expect("it compiles despite the warning");
        script.update(&mut scene, node, 1.0).expect("one frame");
        assert_eq!(position(&scene, node).x, 2.0);
    }

    #[test]
    fn start_sees_the_stored_value_rather_than_the_default() {
        // Ordering: instantiate, write the stored values, then call `start`.
        // A `start` that read the default would be reading a value nobody set.
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        host.instantiate_with(
            "
            @export let offset: Vec2;
            func start() { transform.position = offset; }
            func update(dt: f32) { }
            ",
            &mut scene,
            node,
            &[("offset".into(), Value::Vec2(Vec2::new(3.0, 4.0)))],
        )
        .expect("it compiles");
        assert_eq!(position(&scene, node), Vec2::new(3.0, 4.0));
    }

    #[test]
    fn a_stored_value_the_script_no_longer_declares_is_dropped() {
        // The component and the module can disagree while a source is being
        // edited. Skipping is better than writing a value into the wrong place.
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate_with(
                TUNABLE,
                &mut scene,
                node,
                &[
                    ("speed".into(), Value::F32(5.0)),
                    ("gone".into(), Value::F32(1.0)),
                    ("speed".into(), Value::Bool(true)),
                ],
            )
            .expect("it compiles");
        script.update(&mut scene, node, 1.0).expect("one frame");
        assert_eq!(position(&scene, node).x, 5.0, "the good one still applied");
    }

    #[test]
    fn reconcile_keeps_tuning_across_an_edit_to_the_source() {
        // What ADR 0008's field migration means for a script: a name that is
        // still declared at the same type keeps its value, one that is gone
        // goes, and a new one arrives at its default.
        let mut component = ScriptComponent {
            source: "s.cmt".into(),
            exports: vec![
                ("speed".into(), Value::F32(120.0)),
                ("removed".into(), Value::F32(1.0)),
                ("retyped".into(), Value::F32(2.0)),
            ],
            hints: Vec::new(),
        };
        component.reconcile(&[
            ("speed".into(), Value::F32(0.0)),
            ("retyped".into(), Value::Bool(false)),
            ("added".into(), Value::F32(7.0)),
        ]);
        assert_eq!(
            component.exports,
            vec![
                ("speed".into(), Value::F32(120.0)),
                ("retyped".into(), Value::Bool(false)),
                ("added".into(), Value::F32(7.0)),
            ]
        );
    }

    #[test]
    fn an_exported_variable_becomes_an_inspector_field_without_anyone_being_told() {
        // The inspector and the serializer both walk field_names(), so this is
        // what makes decision 3 load-bearing rather than merely tidy.
        let component = ScriptComponent {
            source: "s.cmt".into(),
            exports: vec![("speed".into(), Value::F32(120.0))],
            hints: Vec::new(),
        };
        assert_eq!(component.field_names(), vec!["source", "speed"]);
        assert_eq!(component.get("speed"), Some(Value::F32(120.0)));

        let mut component = component;
        assert!(component.set("speed", Value::F32(5.0)));
        assert_eq!(component.get("speed"), Some(Value::F32(5.0)));
        // A stale value of the wrong type is refused rather than written.
        assert!(!component.set("speed", Value::Bool(true)));
    }
}
