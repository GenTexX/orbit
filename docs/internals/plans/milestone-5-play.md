# Plan - Milestone 5: Play & Hot Reload

## Context

Press Play, the game runs in the viewport, and saving the script reloads it while preserving the values the inspector owns. This is the milestone the whole architecture is shaped for - ADR 0002's in-process runtime, ADR 0016's one reflection contract, ADR 0008's field migration - and the first one where a person watches their own code do something.

The shape of the work is unusual and worth stating before the steps: **the hard half is built and the easy half does not exist.** comet compiles and runs, `ScriptHost` binds a module to a live node's transform, `start`/`update`/`on_destroy` all exist and are tested, a trap already names the comet function it happened in, and `ScriptComponent::reconcile` is ADR 0008's migration, written and tested. What is missing is everything around it: nothing in atlas has ever constructed a `ScriptHost`, there is no loop, no play/stop state, no input, no camera, no watcher, and `voyager` is six lines with an empty `[dependencies]`.

The good news is specific. atlas already runs `ControlFlow::Poll` with an unconditional redraw, already computes a clamped per-frame `dt` (`main.rs:6036`), and already re-reads the scene into sprites every frame with no Ui rebuild (`main.rs:6110`) - so a script that moves a node will animate the moment something calls `update`, and the call goes between those two lines. The milestone is everything that makes those lines safe and usable around them.

The 2026-08-03 review ([review-2026-08-03.md](../review-2026-08-03.md)) enumerated the gap and closed the prerequisites: the crash floor, the `@export` save/load loss, the container ownership defects, the quadratic gutter and the frame-depth pre-pass are all fixed.

## Example scripts

The north-star game is a one-screen platformer, so these are its scripts, written in the syntax M5 assumes. **Each was compiled against `helios::script_schema()` before this plan was written**, and what follows is what the compiler actually said - not what it was assumed it would say. That is the whole reason this section exists: it turns "M5 needs a language surface" into a list.

### 1. The player - and the single thing that blocks it

```
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
```

Three diagnostics, all the same one: `cannot find 'input' in this scope`. Replace the three `input.*` reads with ordinary `bool` state and **the script compiles clean, today, unchanged in every other respect** - gravity, jumping, the grounded flag, `&&`, compound assignment, assignment into a `Vec2` field, exported tuning values with defaults.

That is the most useful fact in this plan. **Input is the only language work Milestone 5 needs.** Everything else on the wish list - equality on enums, `break`, `find`, `distance`, `time`, `random` - is real but is not on the path to a playable character, and this plan does not do any of it.

### 2. A moving platform - compiles today, no changes

```
@export let travel: f32 = 200.0;
@export let period: f32 = 3.0;
let home = vec2(0.0, 0.0);
let elapsed = 0.0;

func start() { home = transform.position; }

func update(dt: f32) {
    elapsed += dt;
    transform.position.x = home.x + sin(elapsed / period) * travel;
}
```

1633 bytes of WebAssembly. It needs `start` to capture where it was placed, which is exactly what `start` was added for in iteration 4.1, and it needs no host surface beyond the transform.

### 3. A camera that follows - compiles today, no changes

```
@export let smoothing: f32 = 6.0;
let target = vec2(0.0, 0.0);

func update(dt: f32) {
    let to = target - transform.position;
    transform.position += to * (smoothing * dt);
}
```

Vec2 subtraction, scaling by a number, and `+=` on a whole `Vec2`. All of it is iteration 4.1's work. What this script cannot do is find out where the player is - `target` has to be written from outside - which is the honest reason node-to-node access is named in "Deliberately out" below rather than waved at.

### 4. A state machine - compiles, but not the way you would write it

```
enum Mode { Idle, Run }
let mode = Mode::Idle;

func tick_idle(dt: f32) -> f32 { dt * 2.0 }
func tick_run(dt: f32) -> f32 { dt * 4.0 }

func update(dt: f32) {
    let d = match mode { Idle => tick_idle(dt), Run => tick_run(dt) };
    transform.position.x += d;
}
```

That compiles. The version a person actually writes does not:

```
let next = match mode {
    Idle => if speed > 0.0 { Mode::Run } else { Mode::Idle },
    ...
};
```

`if` is a statement, so an `if/else` cannot be an arm's value, and the failure is a four-diagnostic cascade led by `this 'match' does not cover 'Run', 'Jump'` - which blames the wrong thing entirely. **This is not M5 work, but the misleading message is a small fix and it belongs on the list**, because a state machine is what a platformer's player script becomes about twenty minutes after it first moves.

### 5. The goal - deliberately not attempted

```
let player = find("Player");
if distance(transform.position, player.transform.position) < radius { ... }
```

`cannot find function 'find'`, `cannot find function 'distance'`. Node-to-node access is a design question with several answers (a name lookup, a stable node path, an exported node reference on the component) and none of them is needed for a character that runs and jumps. For M5 the goal, if there is one, is a check the player's own script makes against a tuned `@export`ed position.

## Decisions carried in

Taken 2026-08-03, recorded in [the roadmap](../roadmap.md).

- **Hot reload means the file changed on disk and the game noticed** - a save, or an edit in another window, found by an mtime poll. *Typing in the Code pane while the game runs in the same window* is explicitly not M5: it turns the in-place inspector refresh, script output without a Ui rebuild, and the input seam from polish into prerequisites, and roughly doubles the milestone.
- **The north-star game is a one-screen platformer**, and it is the scope filter: M5 builds the components that game needs and no others.
- **Play runs on a clone of the scene, restored on Stop.** `Scene` derives `Clone` (`scene.rs:49`) and slotmap keys survive a clone, so selection, `tree_collapsed` and `inspector_collapsed` all stay valid. Not through RON: `Scene::from_ron` renumbers every `NodeId`.
- **Play saves the open script first**, and the reload path never writes into the Code pane's buffer - it only recompiles. A watcher that wrote into `script_text` would be a fourth instance of the mirror bug this project has had three times.
- **Epoch interruption, not fuel.** Epochs cost nothing when nothing fires, and the failure being guarded is "one frame took forever", which is a wall-clock question.
- **A hot reload does not re-run `start`.** A script whose `start` sets the position would teleport the node back on every save. ADR 0008 is amended to say so.
- **A reload preserves the component's values, not the running module's.** ADR 0022 makes the inspector the owner. A script that mutates its own exported variable at runtime loses that on reload; it must be written down.
- **The loop lives in voyager from the first commit.** ADR 0002 says the editor links the runtime library and the shipping binary is a thin wrapper around the same library. Everything here would fit into `atlas::State` easily, which is exactly why it must not go there.
- **Instances are keyed by `(NodeId, component index)`**, because a node may run several scripts, and the update order is scene pre-order.

## Ground truth checked before planning this

Every one of these was verified in the repository rather than remembered.

- The example scripts above were compiled against `helios::script_schema()`. Two compile unchanged, one needs only `input`, one needs a rephrasing, one is out of scope.
- `helios::ScriptHost` has no caller outside its own tests; atlas uses comet only for the language service.
- `dt` exists and is clamped at `crates/atlas/src/main.rs:6036`; the scene is read into sprites at `:6110`. The insertion point is between them.
- `Scene` derives `Clone` at `crates/helios/src/scene.rs:49`.
- `crates/voyager/Cargo.toml` has an empty `[dependencies]` section. voyager is an isolated node in the dependency graph.
- `ScriptState` holds a `Transform` and a list of printed lines and nothing else. The host property accessors end in catch-all arms, so a row added to `PROPERTIES` without its matching arm compiles and silently reads zero - the failure that bit three times during 4.x.
- There is no file watcher and no `notify` dependency. The precedent is the theme's 400 ms mtime poll in `spectrum::settings`, driven by `State::poll_settings`.
- atlas has never observed a key *release*: the `KeyboardInput` arm is guarded on `ElementState::Pressed`, and `KeyEvent::repeat` is never consulted.
- aurora's `InputEvent` has no release event and its `Key` enum is 22 text-editing verbs, so game input cannot route through aurora's focus model. It is a second path from winit.

## Scope

**In:** an execution budget; a `voyager::Runtime` that owns and steps instances; play/stop with a scene snapshot; a Camera component; an input state a script can read; script output and traps reaching the editor; an mtime watcher and the reload swap.

**Out, deliberately:** a fixed timestep; pause and step-a-frame; a named action map (v1 is a fixed set of schema properties); node-to-node access, `find`, and `distance`; script-driven spawn and despawn; `on_destroy` on node deletion; the Add Component picker; play-in-a-separate-window; and typing in the Code pane during Play.

## Crate layout

The dependency direction is the part worth getting right, because M6 depends on it.

- **helios** gains the `Camera` component and the input state a script reads, plus the budget on `ScriptHost`. It gains no loop and no notion of playing.
- **voyager** becomes a real library: `Runtime` owning a `ScriptHost`, the instance map, the input state, and `step(&mut Scene, dt)`. It depends on `helios` and nothing else - notably not on `photon`, because stepping a scene is not drawing one. Its existing `main.rs` stays a stub until M6.
- **atlas** depends on `voyager` and owns only what is editor-specific: the play/stop state, the scene snapshot, the toolbar, and routing winit input into the runtime's input state.
- **comet** is untouched by this milestone except for whatever the input surface needs, which is nothing: ADR 0020 means input arrives as schema rows.

## Ordered steps (each about one commit)

### Part A - make it safe to run anything

1. **An execution budget.** `Engine` with `epoch_interruption`, a deadline set per `update`, and a `stopped` flag on `ScriptInstance` so one bad frame does not error every frame afterwards. Turn the trap into a console line with a source position via `comet::service::function_line`.
2. **Resource limits.** `StoreLimits` on the store and a `maximum` on comet's memory, so a script that allocates in a loop fails as a script rather than as the editor.

### Part B - voyager becomes real

3. **`voyager::Runtime`** owning the host, the `(NodeId, component index)` instance map, an input state, and `step`. Fifty lines that only tick transforms is enough to start.
4. **Lifecycle**: instantiate on Play in pre-order, `start` once, `update` per frame, `destroy` on Stop. A script that fails to compile is reported and skipped, never fatal.

### Part C - the editor can press it

5. **Play/stop state in atlas**, the scene cloned on Play and restored on Stop, and a gate on every mutating path: the gizmo grab and pick, the drag apply, delete/duplicate/paste, the inspector commits, and the history shortcuts.
6. **Toolbar buttons, and a viewport that says it is playing.** In-process Play leaves the editing UI live and clickable, so the mode has to be visible or a drag during Play reads as lost work when Stop reverts it.
7. **Call `runtime.step(dt)` in the frame loop**, between `main.rs:6036` and `:6110`. *This is the demo*: the bouncing sprite moves.
8. **Script output to the console** by draining `take_printed_tagged`, without setting `dirty` - `poll_console` currently sets it on any log change, which during Play would rebuild the shell several times a second under the user's hands.
9. **The inspector refreshes in place** during Play through the existing `sync_inspector_transform` mechanism, extended past the transform.

### Part D - a game, not an animation

10. **A `Camera` component**, and a Play view that comes from the scene, falling back to the editor camera when there is none so Play demoes before anyone has added one.
11. **Key releases at the winit seam.** A `Released` arm and `KeyEvent::repeat`, held keys cleared on focus loss, feeding an input state atlas owns and hands to the runtime. This is a prerequisite for the next step and has no other consumer today.
12. **Input in the host schema**: `input.left/right/up/down/jump` as bools and `input.mouse` as a `Vec2` - rows in `PROPERTIES` and arms in the accessors, nothing in the compiler. **Add the accessor arm in the same commit as the row**; the catch-all arms make a missing one silent.

### Part E - hot reload

13. **An mtime poll** for the project's `.cmt` files, modelled on `poll_settings`.
14. **The swap**: `forget` the module, recompile, `reconcile` the component's values, instantiate with them, replace the instance. A source that no longer compiles leaves the old instance running and reports - a broken save must not stop the game.

### Not in the steps, but on the list

- The misleading `match` diagnostic when an arm's value is an `if/else` (example script 4). One or two hours, and it is the error a person meets the first time they write a state machine.

## Testing strategy

The runtime is headless by construction, which is the point of it living in voyager: a `Runtime` over a `Scene` with a fake host needs no window and no GPU, so Parts A, B and E are ordinary tests. Worth pinning specifically:

- a runaway script traps on its deadline rather than hanging, and is not called again;
- Play then Stop leaves the scene's RON byte-identical to what it was before;
- a reload keeps a tuned `@export` value and does not re-run `start`;
- a reload of a source that does not compile leaves the previous instance running;
- update order is scene pre-order, and a node with two scripts runs both;
- the player script from example 1, driven by a synthetic input state, moves and jumps as expected - which is the milestone's proof point in the same way "a node moves" was Milestone 4's.

The parts that need a person are the ones that always do: whether the play mode reads as a mode, and whether the reload feels immediate.

## Verification

The milestone is done when the demo project's sprite moves on Play, stops and reverts on Stop, and editing `bounce.cmt` and saving it changes what the sprite does without restarting - with the tuned `@export` values still where the inspector left them. The stretch, and the thing that makes the north star real, is example script 1 running on a node you can drive with the arrow keys.

## Open questions

Two, and both can be answered while Part A is being built.

1. **Is input polled state or events?** This plan assumes polled state - `input.left` is true while the key is held - because that is what a character controller wants and what a fixed schema can express. An event surface (`on_key_down`) would need a callback mechanism the language does not have. Confirming this closes step 12's design.
2. **Does Stop restore the scene, or keep what the game did?** This plan says restore, on the grounds that Play is a rehearsal and the authored scene is the document. The alternative - keep, and let undo revert it - is what some engines do and would need script writes to go through `History`, which they currently do not.
