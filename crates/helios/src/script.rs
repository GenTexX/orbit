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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use glam::Vec2;
use thiserror::Error;
use wasmtime::{
    Caller, Config, Engine, Extern, Instance, Linker, Module, Store, StoreLimits,
    StoreLimitsBuilder, TypedFunc, Val,
};

use crate::input::Input;
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

/// How long one call into a script may run before it is interrupted.
///
/// Generous on purpose. A frame is 16ms and a script that needs a tenth of a
/// second for one `update` is already a performance problem, but this is not a
/// performance tool - it is the difference between a mistake costing a frame
/// and a mistake costing the session, so it is set where nothing correct can
/// reach it and every runaway loop does.
pub const FRAME_BUDGET: Duration = Duration::from_millis(100);

/// How often the epoch advances. The deadline is counted in ticks, so this is
/// also the granularity of [`FRAME_BUDGET`]: a script overruns by up to one
/// tick before it is stopped.
const TICK: Duration = Duration::from_millis(10);

/// [`FRAME_BUDGET`] in epoch ticks, which is the unit a deadline is set in.
fn budget_ticks() -> u64 {
    (FRAME_BUDGET.as_nanos() / TICK.as_nanos().max(1)).max(1) as u64
}

/// The most memory one script may occupy.
///
/// The same number comet declares as its module's maximum, deliberately: the
/// module's limit is what the compiler promises and this is what the host
/// enforces, and they agree so that neither fires before the other. A script
/// that reaches it gets comet's trap, which names the function that allocated;
/// a module from somewhere else, or a comet that later raises its own maximum,
/// still stops here.
///
/// Time and memory are the same argument (ADR 0002 runs the game in the
/// editor's process), but not the same failure: a loop with no exit costs a
/// session, while an allocation with no bound costs whatever else the machine
/// was doing.
pub const MAX_SCRIPT_MEMORY: usize = comet::MAX_PAGES as usize * 64 * 1024;

/// Whether a script being brought up is starting or being swapped in under a
/// game that is already running.
///
/// The difference is one call, and it is ADR 0008's, amended when hot reload was
/// designed: a reload does not re-run `start`. A script whose `start` places the
/// node - which is what `start` is for - would teleport it back to its opening
/// position on every save, and the person watching would conclude that saving
/// resets the game.
///
/// The state a `start` would have set is not recovered. A reloaded script begins
/// its first frame with its declarations at their initializers and its exported
/// variables at what the component holds (ADR 0022), which is the same thing a
/// reload preserves everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begin {
    /// Run `start()` before the first frame.
    Fresh,
    /// Skip it: something was already running here.
    Reload,
}

/// The engine and compiled-module cache shared by every script in a project.
///
/// One of these per editor or game: an [`Engine`] is expensive to build and
/// modules are only usable with the one that compiled them.
pub struct ScriptHost {
    engine: Engine,
    linker: Linker<ScriptState>,
    modules: HashMap<PathBuf, Module>,
    /// Tells the ticker thread to stop when this host goes away.
    ticking: Arc<AtomicBool>,
}

impl Drop for ScriptHost {
    fn drop(&mut self) {
        self.ticking.store(false, Ordering::Relaxed);
    }
}

impl ScriptHost {
    /// A host with the comet imports bound and a budget on every call.
    ///
    /// ## Why a thread
    ///
    /// `while true { }` compiles cleanly, and `while` is comet's only loop, so
    /// forgetting the increment is one keystroke. ADR 0002 runs the game in the
    /// editor's own process, which means an uninterruptible script is not a
    /// hung script - it is a hung editor, holding the user's unsaved scene.
    ///
    /// wasmtime offers two ways to interrupt one. Fuel counts instructions,
    /// which is deterministic but taxes every block and needs recalibrating per
    /// machine. Epochs cost nothing at all until the deadline is checked, and
    /// the question being asked here is a wall-clock one - "has this frame gone
    /// on too long" - so epochs are the honest fit.
    ///
    /// The catch is that an epoch only advances when somebody advances it, and
    /// the somebody cannot be the frame loop: if a script is spinning, the frame
    /// loop is exactly what is not running. So the host owns a thread that does
    /// nothing but tick, and it is the first thread in this workspace. It sleeps
    /// more than 99% of the time and touches one atomic.
    pub fn new() -> Result<Self, ScriptError> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(ScriptError::runtime)?;
        let mut linker = Linker::new(&engine);
        bind_host(&mut linker)?;

        let ticking = Arc::new(AtomicBool::new(true));
        std::thread::Builder::new()
            .name("comet-epoch".to_string())
            .spawn({
                // A clone of the engine handle, not of the engine: dropping the
                // host clears the flag and this exits within one tick.
                let engine = engine.clone();
                let ticking = Arc::clone(&ticking);
                move || {
                    while ticking.load(Ordering::Relaxed) {
                        std::thread::sleep(TICK);
                        engine.increment_epoch();
                    }
                }
            })
            .map_err(ScriptError::runtime)?;

        Ok(Self {
            engine,
            linker,
            modules: HashMap::new(),
            ticking,
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
        self.start(&module, scene, node, exports, Begin::Fresh)
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
        begin: Begin,
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
        let mut instance = self.start(&module, scene, node, exports, begin)?;
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
        begin: Begin,
    ) -> Result<ScriptInstance, ScriptError> {
        let mut store = Store::new(
            &self.engine,
            ScriptState {
                transform: scene.node(node).transform,
                input: Input::default(),
                elapsed: 0.0,
                frame: 0.0,
                rng: 0x2545_F491,
                printed: Vec::new(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(MAX_SCRIPT_MEMORY)
                    .build(),
            },
        );
        // Only memory is capped. Table and instance counts are fixed by what
        // comet emits, so a limit on them could only ever be tripped by a
        // change to the compiler - which is a bug to fix, not a script to stop.
        store.limiter(|state| &mut state.limits);
        // Armed before anything runs, because instantiation itself runs code:
        // the module's wasm start function is where a script's state
        // initializers evaluate, and with epoch interruption on, a store whose
        // deadline nobody set traps on the first instruction.
        store.set_epoch_deadline(budget_ticks());
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
        //
        // Budgeted like every other call: a loop with no exit in `start` would
        // otherwise hang the editor at the moment a script is attached, which
        // is worse than hanging it at Play - the user has not pressed anything
        // yet.
        if begin == Begin::Fresh
            && let Ok(start) = instance.get_typed_func::<(), ()>(&mut store, START)
        {
            store.set_epoch_deadline(budget_ticks());
            start.call(&mut store, ()).map_err(ScriptError::trap)?;
        }
        scene.node_mut(node).transform = store.data().transform;
        Ok(ScriptInstance {
            store,
            instance,
            update,
            on_destroy,
            label: scene.node(node).name.clone(),
            stopped: false,
        })
    }
}

/// One script running for one node: its own memory, globals, and state, which
/// outlive each call and are what makes a script's `let` at the top level
/// persistent.
pub struct ScriptInstance {
    store: Store<ScriptState>,
    /// Kept so exported globals can be read back after `start` - the typed
    /// entry points above are resolved once, but a global is looked up by name
    /// whenever somebody asks what the script currently holds.
    instance: Instance,
    update: Option<TypedFunc<f32, ()>>,
    on_destroy: Option<TypedFunc<(), ()>>,
    /// What this instance's printed output is tagged with.
    label: String,
    /// Set when a call trapped. A trapped instance is not called again.
    ///
    /// Without this the caller gets the same error every frame forever, which
    /// is sixty identical console lines a second and no way to read the first
    /// one - and the first one is the only one that says anything. A wasm trap
    /// also leaves the store's memory in whatever state the trap found it, so
    /// continuing to call in would be running a script whose state is
    /// arbitrary. Stopping is both kinder and more honest.
    stopped: bool,
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
        if self.stopped {
            return Ok(());
        }
        let state = self.store.data_mut();
        state.transform = scene.node(node).transform;
        // Before the call, so the first frame reads a `dt`'s worth of elapsed
        // time rather than zero - a script that divides by `time.elapsed` on
        // frame one should not meet an infinity.
        state.elapsed += dt;
        state.frame += 1.0;
        self.arm();
        let result = update.call(&mut self.store, dt);
        // Written back even when the call trapped: whatever the script managed
        // before it ran out of time is where the node actually is, and throwing
        // that away would make a trapped frame look like a frame that never ran.
        scene.node_mut(node).transform = self.store.data().transform;
        self.settle(result)
    }

    /// Whether this instance trapped and is no longer being called.
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Let it run again after a trap - what a hot reload does, since the new
    /// module gets a fresh store and deserves a fresh chance.
    pub fn resume(&mut self) {
        self.stopped = false;
    }

    /// Set what this script will read from `input` on its next call.
    ///
    /// Copied in rather than read out through a callback, the same way the
    /// transform is: a frame's input is a fixed thing while that frame runs, so
    /// two reads of `input.left` in one `update` cannot disagree.
    pub fn set_input(&mut self, input: Input) {
        self.store.data_mut().input = input;
    }

    /// Give the next call its budget.
    fn arm(&mut self) {
        self.store.set_epoch_deadline(budget_ticks());
    }

    /// Record whether a call trapped, and dress a limit up as the sentence a
    /// person can act on.
    fn settle(&mut self, result: Result<(), wasmtime::Error>) -> Result<(), ScriptError> {
        let Err(err) = result else {
            return Ok(());
        };
        self.stopped = true;
        let limit = Limit::behind(&err);
        let error = ScriptError::trap(err);
        // Everything else is the script's own mistake, and wasmtime's wording
        // for those - "integer divide by zero" - is already the right one.
        let Some(limit) = limit else {
            return Err(error);
        };
        Err(match error.trapped_in() {
            Some(function) => ScriptError::Trapped {
                function: function.to_string(),
                message: limit.sentence(),
            },
            None => ScriptError::Runtime(format!("a script {}", limit.sentence())),
        })
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
        // Runs even on a stopped instance: a trap earlier is no reason to skip
        // the hook that gives a script its last word, and it gets its own
        // budget so a loop in the hook cannot hang the teardown either.
        self.store.data_mut().transform = scene.node(node).transform;
        self.arm();
        let result = on_destroy.call(&mut self.store, ());
        scene.node_mut(node).transform = self.store.data().transform;
        self.settle(result)
    }

    /// Write one exported variable into this running script.
    ///
    /// The write half of `read_exports`, and the thing that turns Play from a
    /// rehearsal you watch into one you can turn the dials on: drag `gravity`
    /// and the jump changes under your hand, rather than stop, edit, play, and
    /// lose your position.
    ///
    /// A live override, not an edit. ADR 0022 keeps the component the owner, so
    /// the authored value is untouched and Stop puts it back - which is also why
    /// this needs no undo entry and cannot lose anybody's work.
    pub fn write_export(&mut self, name: &str, value: &Value) {
        let instance = self.instance;
        write_export(&mut self.store, &instance, name, value);
    }

    /// Carry this script's plain top-level state over from `previous`.
    ///
    /// What a hot reload should feel like: your code changed and your game kept
    /// going. Without it a reload re-runs every top-level initializer but not
    /// `start` - the two places a script sets things up have opposite reload
    /// semantics - so a script that assigns its velocity in `start` simply
    /// stops when you save the file. That is the milestone's own verification
    /// case producing a broken sprite.
    ///
    /// Only what comet exported as carryable, which is scalars: a reload builds
    /// a whole new linear memory, so a `String` or an `Array` would arrive as a
    /// pointer into a heap that no longer exists. Those restart cold, which is
    /// what ADR 0008 says happens to heap object graphs.
    ///
    /// `owned` names the variables the component owns (ADR 0022) and is skipped:
    /// the inspector's value has already been written in, and the running
    /// module's copy must not win over it.
    pub fn carry_state_from(&mut self, previous: &mut ScriptInstance, owned: &[String]) {
        let skip: Vec<String> = owned
            .iter()
            .flat_map(|name| {
                [
                    format!("state.{name}"),
                    format!("state.{name}.x"),
                    format!("state.{name}.y"),
                ]
            })
            .collect();
        let names: Vec<String> = self
            .instance
            .exports(&mut self.store)
            .filter(|export| export.name().starts_with("state."))
            .map(|export| export.name().to_string())
            .collect();
        for name in names {
            if skip.contains(&name) {
                continue;
            }
            let Some(from) = previous.instance.get_global(&mut previous.store, &name) else {
                continue;
            };
            let Some(into) = self.instance.get_global(&mut self.store, &name) else {
                continue;
            };
            let value = from.get(&mut previous.store);
            // A name that changed type between the two versions is a different
            // variable wearing the same name, so it starts fresh rather than
            // being forced.
            if into
                .ty(&self.store)
                .content()
                .matches(from.ty(&previous.store).content())
            {
                let _mismatch = into.set(&mut self.store, value);
            }
        }
    }

    /// Read back what the running script currently holds for each of
    /// `declared`, in the same order.
    ///
    /// The mirror of `apply_exports`, and it exists for one reason: a script may
    /// assign to its own exported variable, and a value that changes every frame
    /// with nowhere to show it is a value nobody can debug. An inspector can
    /// draw these while a game runs.
    ///
    /// A readout, not ownership. ADR 0022 keeps the component the owner of these
    /// values; this says what the module has right now, which is a different
    /// question with a different answer the moment a script assigns.
    ///
    /// A name the module does not export, or exports at another type, keeps the
    /// value it was given - the same rule `apply_exports` uses in the other
    /// direction, and for the same reason: the component and the module are
    /// reconciled elsewhere, and this is not the place to decide they disagree.
    pub fn read_exports(&mut self, declared: &[(String, Value)]) -> Vec<(String, Value)> {
        declared
            .iter()
            .map(|(name, value)| (name.clone(), self.read_export(name, value)))
            .collect()
    }

    fn read_export(&mut self, name: &str, value: &Value) -> Value {
        let globals = match value {
            Value::Vec2(_) => comet::exported_globals(name, comet::Type::Vec2),
            Value::F32(_) => comet::exported_globals(name, comet::Type::F32),
            Value::Bool(_) => comet::exported_globals(name, comet::Type::Bool),
            Value::Int(_) => comet::exported_globals(name, comet::Type::Int),
            _ => return value.clone(),
        };
        let mut read = Vec::with_capacity(globals.len());
        for global in &globals {
            let Some(found) = self.instance.get_global(&mut self.store, global) else {
                return value.clone();
            };
            read.push(found.get(&mut self.store));
        }
        match (value, read.as_slice()) {
            (Value::F32(_), [Val::F32(bits)]) => Value::F32(f32::from_bits(*bits)),
            (Value::Int(_), [Val::I32(v)]) => Value::Int(*v),
            (Value::Bool(_), [Val::I32(v)]) => Value::Bool(*v != 0),
            (Value::Vec2(_), [Val::F32(x), Val::F32(y)]) => {
                Value::Vec2(Vec2::new(f32::from_bits(*x), f32::from_bits(*y)))
            }
            _ => value.clone(),
        }
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

/// Which limit stopped a script, as opposed to the script stopping itself.
///
/// Both of these arrive as traps, and both of wasmtime's words for them -
/// "interrupt", "unreachable" - describe the mechanism rather than the
/// mistake. Naming the two cases is what lets the message describe the mistake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Limit {
    /// [`FRAME_BUDGET`] ran out: the epoch deadline passed mid-call.
    Time,
    /// [`MAX_SCRIPT_MEMORY`] ran out: `memory.grow` was refused, so comet's
    /// allocator trapped rather than hand back a pointer to nothing.
    Memory,
}

impl Limit {
    /// Which limit an error is, if it is one.
    fn behind(err: &wasmtime::Error) -> Option<Self> {
        match err.downcast_ref::<wasmtime::Trap>()? {
            wasmtime::Trap::Interrupt => Some(Limit::Time),
            // An `unreachable` is only out of memory when it is the one inside
            // `comet_alloc`. The innermost frame is what tells them apart -
            // every other `unreachable` comet emits is a bounds check or a
            // `match` that ran out of arms, and those are the script's.
            wasmtime::Trap::UnreachableCodeReached => err
                .downcast_ref::<wasmtime::WasmBacktrace>()
                .and_then(|backtrace| backtrace.frames().first()?.func_name())
                .is_some_and(|name| name == "comet_alloc")
                .then_some(Limit::Memory),
            _ => None,
        }
    }

    /// What happened, phrased so it reads both on its own after "a script" and
    /// with "(in `update`)" behind it.
    fn sentence(self) -> String {
        match self {
            Limit::Time => format!(
                "ran for more than {}ms and was stopped - is there a loop with no way out?",
                FRAME_BUDGET.as_millis()
            ),
            Limit::Memory => format!(
                "ran out of memory after {}MB and was stopped - is something being added \
                 to in a loop?",
                MAX_SCRIPT_MEMORY / (1024 * 1024)
            ),
        }
    }
}

/// What a running script can see and touch.
#[derive(Debug)]
struct ScriptState {
    transform: Transform,
    /// Copied in before every call, like the transform. A script reads it and
    /// never writes it.
    input: Input,
    /// How long this instance has been running, and how many frames it has
    /// seen. Advanced by the host rather than by the script, so a cooldown does
    /// not need a hand-rolled accumulator in a state variable.
    elapsed: f32,
    frame: f32,
    /// The seeded generator `random()` draws from.
    ///
    /// Per instance and seeded from a fixed constant, which is a teaching
    /// artifact rather than an oversight: the same script started twice plays
    /// the same game, so "why did it do something different" has an answer that
    /// is about the code. A host that wants variety seeds it from the clock.
    rng: u32,
    printed: Vec<String>,
    /// Read through the store's limiter, so it has to live where the store can
    /// hand out a mutable reference to it.
    limits: StoreLimits,
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
        write_export(store, instance, name, value);
    }
}

/// Write one exported variable into the module's globals.
///
/// The half of `apply_exports` that does the work, split out because it has a
/// second caller now: the inspector, writing a value into a script that is
/// already running. That is the difference between Play being a rehearsal you
/// watch and one you can turn the dials on.
fn write_export(store: &mut Store<ScriptState>, instance: &Instance, name: &str, value: &Value) {
    let globals = match value {
        Value::Vec2(_) => comet::exported_globals(name, comet::Type::Vec2),
        Value::F32(_) => comet::exported_globals(name, comet::Type::F32),
        Value::Bool(_) => comet::exported_globals(name, comet::Type::Bool),
        // comet's `Int` and a payload-free enum's tag are both one i32 global,
        // and `exported_globals` names them the same way, so one arm covers
        // both.
        Value::Int(_) => comet::exported_globals(name, comet::Type::Int),
        _ => return,
    };
    let wasm: Vec<Val> = match value {
        Value::F32(v) => vec![Val::F32(v.to_bits())],
        Value::Int(v) => vec![Val::I32(*v)],
        Value::Bool(v) => vec![Val::I32(i32::from(*v))],
        Value::Vec2(v) => vec![Val::F32(v.x.to_bits()), Val::F32(v.y.to_bits())],
        _ => return,
    };
    for (name, value) in globals.iter().zip(wasm) {
        let Some(global) = instance.get_global(&mut *store, name) else {
            continue;
        };
        // A failure here is a real disagreement between the component and the
        // module - the value's type is not the global's - and used to be thrown
        // away with `let _`, which is how an exported int came to be editable,
        // saved, and silently inert. helios takes no logging dependency (that
        // is the editor's), so it is dropped deliberately and named rather than
        // swallowed anonymously.
        let _mismatch = global.set(&mut *store, value);
    }
}

/// Which property a schema id refers to. The order here is what fixes the ids,
/// and [`schema`] is built from the same list, so the two cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Property {
    Position,
    Rotation,
    Scale,
    InputLeft,
    InputRight,
    InputUp,
    InputDown,
    InputJump,
    InputMouse,
    TimeElapsed,
    TimeFrame,
}

/// Everything a script can reach outside itself, and what each thing is.
///
/// comet compiles against the schema this produces and never learns what a
/// `Transform` is. Adding a property here makes it scriptable - with
/// completions and hover - without touching the compiler.
/// Shorthands, so a row stays one line and the table stays readable.
const RW: comet::Access = comet::Access::ReadWrite;
const RO: comet::Access = comet::Access::ReadOnly;

const PROPERTIES: &[(&str, &str, comet::HostType, comet::Access, Property)] = &[
    (
        "transform",
        "position",
        comet::HostType::Vec2,
        RW,
        Property::Position,
    ),
    (
        "transform",
        "rotation",
        comet::HostType::F32,
        RW,
        Property::Rotation,
    ),
    (
        "transform",
        "scale",
        comet::HostType::Vec2,
        RW,
        Property::Scale,
    ),
    // Input is read-only: it is the engine's answer to what the player is
    // doing, so a script assigning to it would be telling the keyboard what was
    // pressed. Declared rather than merely ignored - the checker now refuses it
    // with a sentence, where before it compiled, emitted a setter, and was
    // silently dropped here.
    (
        "input",
        "left",
        comet::HostType::Bool,
        RO,
        Property::InputLeft,
    ),
    (
        "input",
        "right",
        comet::HostType::Bool,
        RO,
        Property::InputRight,
    ),
    ("input", "up", comet::HostType::Bool, RO, Property::InputUp),
    (
        "input",
        "down",
        comet::HostType::Bool,
        RO,
        Property::InputDown,
    ),
    (
        "input",
        "jump",
        comet::HostType::Bool,
        RO,
        Property::InputJump,
    ),
    (
        "input",
        "mouse",
        comet::HostType::Vec2,
        RO,
        Property::InputMouse,
    ),
    // The clock, read-only for the same reason input is: the engine is what
    // knows what time it is. Without these, "three seconds after start" needs a
    // hand-rolled accumulator in a state variable.
    (
        "time",
        "elapsed",
        comet::HostType::F32,
        RO,
        Property::TimeElapsed,
    ),
    (
        "time",
        "frame",
        comet::HostType::F32,
        RO,
        Property::TimeFrame,
    ),
];

/// Read a property the script asked for as an `f32`.
///
/// Every accessor below is exhaustive over [`Property`], which is the whole
/// point of them being functions rather than arms inside a closure. They used to
/// end in `_ => 0.0`, and a row added to `PROPERTIES` without its arm compiled
/// cleanly and silently read zero - a trap that sprang three times during 4.x.
/// Now the compiler names every accessor that has not been told about a new
/// property.
///
/// A property of another type reaching here means a module compiled against a
/// different schema than the one it is running on, so it reads as a default
/// rather than as an error: the values are already meaningless, and a trap would
/// blame the script for the host's version skew.
fn read_f32(state: &ScriptState, property: Property) -> f32 {
    match property {
        Property::Rotation => state.transform.rotation,
        Property::TimeElapsed => state.elapsed,
        Property::TimeFrame => state.frame,
        Property::Position
        | Property::Scale
        | Property::InputMouse
        | Property::InputLeft
        | Property::InputRight
        | Property::InputUp
        | Property::InputDown
        | Property::InputJump => 0.0,
    }
}

fn write_f32(state: &mut ScriptState, property: Property, value: f32) {
    match property {
        Property::Rotation => state.transform.rotation = value,
        Property::Position
        | Property::Scale
        | Property::InputMouse
        | Property::InputLeft
        | Property::InputRight
        | Property::InputUp
        | Property::InputDown
        | Property::InputJump
        | Property::TimeElapsed
        | Property::TimeFrame => {}
    }
}

fn read_bool(state: &ScriptState, property: Property) -> bool {
    match property {
        Property::InputLeft => state.input.left,
        Property::InputRight => state.input.right,
        Property::InputUp => state.input.up,
        Property::InputDown => state.input.down,
        Property::InputJump => state.input.jump,
        Property::Position
        | Property::Rotation
        | Property::Scale
        | Property::InputMouse
        | Property::TimeElapsed
        | Property::TimeFrame => false,
    }
}

fn write_bool(_state: &mut ScriptState, property: Property, _value: bool) {
    match property {
        // The input is the host's answer to "what is the player doing". A
        // script writing to it would be telling the keyboard what was pressed.
        Property::InputLeft
        | Property::InputRight
        | Property::InputUp
        | Property::InputDown
        | Property::InputJump
        | Property::Position
        | Property::Rotation
        | Property::Scale
        | Property::InputMouse
        | Property::TimeElapsed
        | Property::TimeFrame => {}
    }
}

fn read_vec2(state: &ScriptState, property: Property) -> Vec2 {
    match property {
        Property::Position => state.transform.translation,
        Property::Scale => state.transform.scale,
        Property::InputMouse => state.input.mouse,
        Property::Rotation
        | Property::InputLeft
        | Property::InputRight
        | Property::InputUp
        | Property::InputDown
        | Property::InputJump
        | Property::TimeElapsed
        | Property::TimeFrame => Vec2::ZERO,
    }
}

fn write_vec2(state: &mut ScriptState, property: Property, value: Vec2) {
    match property {
        Property::Position => state.transform.translation = value,
        Property::Scale => state.transform.scale = value,
        Property::InputMouse
        | Property::Rotation
        | Property::InputLeft
        | Property::InputRight
        | Property::InputUp
        | Property::InputDown
        | Property::InputJump
        | Property::TimeElapsed
        | Property::TimeFrame => {}
    }
}

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
        let fields: Vec<(&'static str, comet::HostType, comet::Access)> = PROPERTIES[at..]
            .iter()
            .take_while(|(o, ..)| *o == object)
            .map(|(_, field, ty, access, _)| (*field, *ty, *access))
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

/// Allocate a `String` in the module's heap and write `text` into it.
///
/// The one host function shape that allocates: it calls back into the module
/// rather than reserving memory of its own, so what it returns is an ordinary
/// refcounted block the script releases like any other String. Shared by all
/// four `str` bindings, because four copies of it would be four places to get
/// the ownership wrong.
fn make_string(c: &mut Caller<'_, ScriptState>, text: &str) -> Result<i32, wasmtime::Error> {
    let Some(Extern::Func(alloc)) = c.get_export("comet_alloc") else {
        return Err(wasmtime::Error::msg(
            "every comet module exports comet_alloc",
        ));
    };
    let alloc = alloc.typed::<i32, i32>(&*c)?;
    let ptr = alloc.call(&mut *c, text.len() as i32)?;
    let Some(Extern::Memory(memory)) = c.get_export("memory") else {
        return Err(wasmtime::Error::msg(
            "every comet module exports its memory",
        ));
    };
    comet::write_str(memory.data_mut(&mut *c), ptr, text);
    Ok(ptr)
}

/// Bind the functions every comet module imports (see `comet::HOST_MODULE`
/// - the import list is fixed, so one binding table serves every script).
fn bind_host(linker: &mut Linker<ScriptState>) -> Result<(), ScriptError> {
    let host = comet::HOST_MODULE;
    // The property accessors. Each takes the property's schema id, so this
    // table stays the same size however many properties the engine exposes.
    linker
        .func_wrap(host, "get_f32", |c: Caller<'_, ScriptState>, id: i32| {
            property(id).map_or(0.0, |p| read_f32(c.data(), p))
        })
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "set_f32",
            |mut c: Caller<'_, ScriptState>, id: i32, value: f32| {
                if let Some(p) = property(id) {
                    write_f32(c.data_mut(), p, value);
                }
            },
        )
        .map_err(ScriptError::runtime)?;
    // comet's bools cross the ABI as i32, since wasm has no bool.
    linker
        .func_wrap(host, "get_bool", |c: Caller<'_, ScriptState>, id: i32| {
            property(id).is_some_and(|p| read_bool(c.data(), p)) as i32
        })
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "set_bool",
            |mut c: Caller<'_, ScriptState>, id: i32, value: i32| {
                if let Some(p) = property(id) {
                    write_bool(c.data_mut(), p, value != 0);
                }
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(host, "get_vec2_x", |c: Caller<'_, ScriptState>, id: i32| {
            property(id).map_or(0.0, |p| read_vec2(c.data(), p).x)
        })
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(host, "get_vec2_y", |c: Caller<'_, ScriptState>, id: i32| {
            property(id).map_or(0.0, |p| read_vec2(c.data(), p).y)
        })
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "set_vec2",
            |mut c: Caller<'_, ScriptState>, id: i32, x: f32, y: f32| {
                if let Some(p) = property(id) {
                    write_vec2(c.data_mut(), p, Vec2::new(x, y));
                }
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "str",
            |mut c: Caller<'_, ScriptState>, value: f32| -> Result<i32, wasmtime::Error> {
                make_string(&mut c, &comet::format_f32(value))
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
                make_string(&mut c, &value.to_string())
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "str_bool",
            |mut c: Caller<'_, ScriptState>, value: i32| -> Result<i32, wasmtime::Error> {
                // The words a script would write, so `str(flag)` reads back as
                // what the source says rather than as 1 and 0.
                make_string(&mut c, if value != 0 { "true" } else { "false" })
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(
            host,
            "str_vec2",
            |mut c: Caller<'_, ScriptState>, x: f32, y: f32| -> Result<i32, wasmtime::Error> {
                // Parenthesised, unlike the inspector's bare "x, y": this ends
                // up inside a printed sentence, where two bare numbers separated
                // by a comma cannot be told from two separate values.
                let text = format!("({}, {})", comet::format_f32(x), comet::format_f32(y));
                make_string(&mut c, &text)
            },
        )
        .map_err(ScriptError::runtime)?;
    linker
        .func_wrap(host, "random", |mut c: Caller<'_, ScriptState>| -> f32 {
            // xorshift32, which is nine lines and good enough for a game jam.
            // The point is not the quality of the bits, it is that the host
            // owns the seed: the same script started twice plays the same game,
            // so a learner chasing "why was it different" is chasing their own
            // code rather than the weather.
            let state = c.data_mut();
            state.rng ^= state.rng << 13;
            state.rng ^= state.rng >> 17;
            state.rng ^= state.rng << 5;
            // 24 bits is every value an f32 can hold exactly, so this is
            // uniform rather than nearly so.
            (state.rng >> 8) as f32 / (1u32 << 24) as f32
        })
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
            .instantiate_file(
                Path::new("/nowhere/missing.cmt"),
                &mut scene,
                node,
                &[],
                Begin::Fresh,
            )
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

        let mut script_a = host
            .instantiate_file(&path, &mut scene, a, &[], Begin::Fresh)
            .unwrap();
        let mut script_b = host
            .instantiate_file(&path, &mut scene, b, &[], Begin::Fresh)
            .unwrap();

        // Deleting the file after the first read proves the second instance came
        // from the cache rather than reading it again.
        std::fs::remove_file(&path).unwrap();
        let mut script_c = host
            .instantiate_file(&path, &mut scene, a, &[], Begin::Fresh)
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
            host.instantiate_file(&path, &mut scene, a, &[], Begin::Fresh),
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
        for (id, (object, field, ty, _, _)) in PROPERTIES.iter().enumerate() {
            let found = schema
                .resolve(object, field)
                .unwrap_or_else(|| panic!("{object}.{field} is missing from the schema"));
            assert_eq!(found.id as usize, id, "{object}.{field} is numbered wrong");
            assert_eq!(found.ty, *ty);
            assert_eq!(property(id as i32), Some(PROPERTIES[id].4));
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
    fn a_runaway_script_is_stopped_and_not_called_again() {
        // `while` is comet's only loop and forgetting the increment is one
        // keystroke. ADR 0002 runs the game in the editor's process, so before
        // this the first learner to write one lost their session - the call
        // never returned and there was nothing to interrupt it.
        let source = "
            func update(dt: f32) { while true { } }
        ";
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate(source, &mut scene, node)
            .expect("it compiles and starts");

        let started = std::time::Instant::now();
        let err = script
            .update(&mut scene, node, 0.016)
            .expect_err("it must not run forever");
        let took = started.elapsed();

        // It came back, and it came back for the reason we asked for.
        assert!(
            took < FRAME_BUDGET * 4,
            "stopped near the budget, took {took:?}"
        );
        let message = err.to_string();
        assert!(
            message.contains("loop with no way out"),
            "the message says what to look for: {message}"
        );
        assert_eq!(err.trapped_in(), Some("update"), "and where to look");

        // And it is not called again. Sixty identical console lines a second
        // would bury the one line that says anything.
        assert!(script.is_stopped());
        let second = std::time::Instant::now();
        script
            .update(&mut scene, node, 0.016)
            .expect("a stopped instance is a no-op, not an error");
        assert!(second.elapsed() < FRAME_BUDGET, "and returns immediately");
    }

    #[test]
    fn a_script_that_allocates_without_bound_is_stopped_and_says_so() {
        // The other half of the same argument as the runaway loop above. That
        // one costs the session; this one costs whatever else the machine was
        // doing, because ADR 0002 puts the script's allocator and the editor's
        // in one process.
        //
        // Doubling a string reaches the cap in about two dozen allocations, so
        // memory runs out long before the frame budget does - which is what
        // makes this a test of the memory limit rather than a race with the
        // time limit.
        let source = "
            let text: String = \"x\";
            func update(dt: f32) { while true { text = text + text; } }
        ";
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate(source, &mut scene, node)
            .expect("it compiles and starts");

        let started = std::time::Instant::now();
        let err = script
            .update(&mut scene, node, 0.016)
            .expect_err("it must not allocate forever");
        assert!(
            started.elapsed() < FRAME_BUDGET,
            "memory ran out first, so this is not the time limit in disguise"
        );

        let message = err.to_string();
        assert!(
            message.contains("ran out of memory"),
            "the message says what happened, not `wasm trap: unreachable`: {message}"
        );
        assert!(
            message.contains("added to in a loop"),
            "and what to look for: {message}"
        );
        assert_eq!(err.trapped_in(), Some("update"), "and where to look");
        assert!(script.is_stopped(), "and it is not called again");
    }

    #[test]
    fn a_trap_that_is_the_scripts_own_mistake_keeps_wasmtimes_words() {
        // The out-of-memory message is only right when the trap came from the
        // allocator. comet emits `unreachable` for a bounds check too, and
        // reporting that one as "ran out of memory" would send a learner to
        // look for a leak that is not there.
        let source = "
            let a: Array<f32> = [1.0];
            func update(dt: f32) { let x: f32 = a[3]; }
        ";
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate(source, &mut scene, node)
            .expect("it compiles and starts");

        let message = script
            .update(&mut scene, node, 0.016)
            .expect_err("reading past the end traps")
            .to_string();
        assert!(
            !message.contains("ran out of memory"),
            "an index out of range is not the memory limit: {message}"
        );
    }

    #[test]
    fn a_script_that_finishes_is_never_interrupted() {
        // The budget must be invisible to correct code. This runs the busiest
        // loop a demo script plausibly has, many times over, and none of it
        // comes near a tenth of a second.
        let source = "
            let total = 0.0;
            func update(dt: f32) {
                for i in 0..2000 { total += dt; }
            }
        ";
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate(source, &mut scene, node)
            .expect("it compiles");
        for _ in 0..200 {
            script.update(&mut scene, node, 0.016).expect("no trap");
        }
        assert!(!script.is_stopped());
    }

    #[test]
    fn a_loop_in_start_cannot_hang_the_editor_either() {
        // A script is attached before anything is pressed, and instantiation
        // runs both the state initializers and `start` - so an unbudgeted one
        // hangs at the moment of attaching, which is worse than hanging at Play.
        let source = "
            func start() { while true { } }
            func update(dt: f32) { }
        ";
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let started = std::time::Instant::now();
        let err = host
            .instantiate(source, &mut scene, node)
            .expect_err("it must not hang");
        assert!(
            started.elapsed() < FRAME_BUDGET * 4,
            "stopped near the budget"
        );
        assert_eq!(err.trapped_in(), Some("start"));
    }

    #[test]
    fn an_exported_int_reaches_the_module_it_was_tuned_for() {
        // It did not. `Value` had no Int, so atlas stored an exported int as an
        // F32 and `apply_exports` derived the wasm type from what the component
        // happened to hold - writing an f32 into the module's i32 global.
        // wasmtime refused it and the error was thrown away, so the field was
        // editable, saved, and completely inert: the script ran at the default.
        let source = "
            @export let steps: int;
            func update(dt: f32) { transform.position.x += steps; }
        ";
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate_with(source, &mut scene, node, &[("steps".into(), Value::Int(7))])
            .expect("it compiles");
        script.update(&mut scene, node, 1.0).expect("one frame");
        assert_eq!(position(&scene, node).x, 7.0, "the tuned value ran");

        // And the f32 path still works beside it, from the same table.
        let source = "
            @export let steps: int;
            @export let speed: f32;
            func update(dt: f32) { transform.position.x += steps; \
             transform.position.y += speed; }
        ";
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate_with(
                source,
                &mut scene,
                node,
                &[
                    ("steps".into(), Value::Int(3)),
                    ("speed".into(), Value::F32(1.5)),
                ],
            )
            .expect("it compiles");
        script.update(&mut scene, node, 1.0).expect("one frame");
        assert_eq!(position(&scene, node), Vec2::new(3.0, 1.5));
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

    #[test]
    fn a_script_that_assigns_to_its_own_export_reads_back_what_it_holds() {
        // ADR 0022 makes the component the owner of an exported value, so what
        // the running module holds is a *different* question - and after the
        // script assigns to it, a different answer. An inspector that showed the
        // component's copy during a game would be showing a number the game
        // stopped using on frame one.
        let source = "
            @export let speed: f32 = 1.0;
            @export let steps: int = 0;
            @export let moving: bool = false;
            @export let heading: Vec2;
            func update(dt: f32) {
                speed = speed * 2.0;
                steps = steps + 1;
                moving = true;
                heading = vec2(3.0, 4.0);
            }
        ";
        let declared = vec![
            ("speed".to_string(), Value::F32(5.0)),
            ("steps".to_string(), Value::Int(0)),
            ("moving".to_string(), Value::Bool(false)),
            ("heading".to_string(), Value::Vec2(Vec2::ZERO)),
        ];
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate_with(source, &mut scene, node, &declared)
            .expect("it compiles and starts");

        assert_eq!(
            script.read_exports(&declared),
            declared,
            "before a frame it still holds what the inspector gave it"
        );

        script.update(&mut scene, node, 0.016).expect("one frame");
        assert_eq!(
            script.read_exports(&declared),
            vec![
                ("speed".to_string(), Value::F32(10.0)),
                ("steps".to_string(), Value::Int(1)),
                ("moving".to_string(), Value::Bool(true)),
                ("heading".to_string(), Value::Vec2(Vec2::new(3.0, 4.0))),
            ],
            "and afterwards it holds what the script made of it"
        );
    }

    #[test]
    fn reading_back_a_name_the_script_does_not_export_keeps_what_it_was_given() {
        // The component and the module are reconciled elsewhere. A readout is
        // not the place to decide they disagree, and inventing a zero here
        // would look exactly like a script that had set one.
        let source = "func update(dt: f32) { }";
        let declared = vec![("ghost".to_string(), Value::F32(7.0))];
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate_with(source, &mut scene, node, &declared)
            .expect("it compiles and starts");
        assert_eq!(script.read_exports(&declared), declared);
    }

    #[test]
    fn a_script_can_read_the_clock() {
        // Before this, "three seconds after start" needed a hand-rolled
        // accumulator in a state variable - which is the first thing anybody
        // writes and the first thing they get wrong.
        let source = "
            func update(dt: f32) {
                transform.position.x = time.elapsed;
                transform.position.y = time.frame;
            }
        ";
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let mut script = host
            .instantiate(source, &mut scene, node)
            .expect("it compiles and starts");

        script.update(&mut scene, node, 0.5).expect("one frame");
        assert_eq!(
            position(&scene, node),
            Vec2::new(0.5, 1.0),
            "the first frame has already happened, so neither reads zero"
        );
        script.update(&mut scene, node, 0.25).expect("another");
        assert_eq!(position(&scene, node), Vec2::new(0.75, 2.0));
    }

    #[test]
    fn the_clock_is_read_only() {
        let source = "func update(dt: f32) { time.elapsed = 0.0; }";
        let mut host = ScriptHost::new().expect("a host");
        let (mut scene, node) = scene_with_node(Vec2::ZERO);
        let err = host
            .instantiate(source, &mut scene, node)
            .expect_err("the engine owns the clock");
        assert!(err.to_string().contains("read-only"), "{err}");
    }

    #[test]
    fn random_is_in_range_varies_and_replays() {
        let source = "
            let total = 0.0;
            func update(dt: f32) {
                let r = random();
                if r < 0.0 { transform.position.y = -1.0; }
                if r > 1.0 { transform.position.y = -1.0; }
                total += r;
                transform.position.x = total;
            }
        ";
        let run = || {
            let mut host = ScriptHost::new().expect("a host");
            let (mut scene, node) = scene_with_node(Vec2::ZERO);
            let mut script = host
                .instantiate(source, &mut scene, node)
                .expect("it compiles and starts");
            for _ in 0..64 {
                script.update(&mut scene, node, 0.016).expect("a frame");
            }
            position(&scene, node)
        };
        let first = run();
        assert_eq!(first.y, 0.0, "every draw was inside 0.0 .. 1.0");
        assert!(
            first.x > 8.0 && first.x < 56.0,
            "64 draws averaged somewhere sane: {}",
            first.x
        );

        // The same script started again plays the same game. That is the
        // teaching artifact, not an accident of the generator: a learner asking
        // why it went differently is asking about their own code.
        assert_eq!(run(), first, "seeded per instance, so a run replays");
    }
}
