# Plan - Milestone 5: Play & Hot Reload

## Context

Press Play, the game runs in the viewport, and saving the script reloads it while
preserving the values the inspector owns. This is the milestone the whole architecture is
shaped for - ADR 0002's in-process runtime, ADR 0016's one reflection contract, ADR 0008's
field migration - and the first one where a person can watch their own code do something.

The shape of the work is unusual, and worth stating before the steps: **the hard half is
built and the easy half does not exist.** comet compiles and runs, `ScriptHost` binds a
module to a live node's transform, `start`/`update`/`on_destroy` all exist and are tested, a
trap already names the comet function it happened in, and `ScriptComponent::reconcile` is
ADR 0008's migration, written and tested. What is missing is everything around it: nothing
in atlas has ever constructed a `ScriptHost`, there is no loop, no play/stop state, no
input, no camera, no watcher, and `voyager` is six lines with an empty dependency list.

The good news is specific. atlas already runs `ControlFlow::Poll` with an unconditional
redraw, already computes a clamped per-frame `dt`, and already re-reads the scene into
sprites every frame with no Ui rebuild - so a script that moves a node will animate the
moment something calls `update`. That call is a handful of lines. The milestone is
everything that makes it safe and usable around them.

The 2026-08-03 review ([review-2026-08-03.md](../review-2026-08-03.md)) enumerated the gap
and closed the prerequisites: the crash floor, the `@export` save/load loss, the container
ownership defects, and the quadratic gutter are all fixed, and the frame-depth pre-pass that
caused four unlocated compiler panics is gone. This plan starts from that.

## Decisions carried in

Taken 2026-08-03, recorded in [the roadmap](../roadmap.md).

- **Hot reload means the file changed on disk and the game noticed** - a save, or an edit in
  another window, found by an mtime poll. *Typing in the Code pane while the game runs in
  the same window* is explicitly not M5: it turns the in-place inspector refresh, script
  output without a Ui rebuild, and the input seam from polish into prerequisites, and
  roughly doubles the milestone. It becomes its own iteration afterwards.
- **The north-star game is a one-screen platformer.** It is what Play is first pressed on,
  what M6 exports first, and what the teaching material builds toward. It is also the scope
  filter for which components to build: M5 needs a Camera and nothing else.
- **Play runs on a clone of the scene, restored on Stop.** `Scene` is `Clone` and slotmap
  keys survive a clone, so selection, `tree_collapsed` and `inspector_collapsed` all stay
  valid. Not through RON: `Scene::from_ron` renumbers every `NodeId` and would invalidate
  every handle atlas holds.
- **Play saves the open script first**, and the reload path never writes into the Code
  pane's buffer - it only recompiles. A watcher that writes into `script_text` would be a
  fourth instance of the mirror bug this project has already had three times.
- **Epoch interruption, not fuel.** Fuel is deterministic but taxes every block and needs
  recalibrating per machine; epochs cost nothing when nothing fires, and the failure being
  guarded is "one frame took forever", which is a wall-clock question.
- **A hot reload does not re-run `start`.** A script whose `start` sets the position would
  teleport the node back on every save. `start` runs on Play only. ADR 0008 is amended to
  say so, because it can currently be read either way.
- **A reload preserves the component's values, not the running module's.** ADR 0022 makes
  the inspector the owner. A script that mutates its own exported variable at runtime loses
  that on reload - defensible, but it must be written down, and the codegen comment claiming
  the host can read a global back out goes with it.
- **The loop lives in voyager from the first commit.** ADR 0002 says the editor links the
  runtime library and the shipping binary is a thin wrapper around the same library. There
  is exactly one place where a scene, a renderer, a texture cache and a window all meet -
  `atlas::State` - and everything here would fit there easily, which is precisely why it
  must not go there. Writing it in atlas and extracting it in M6 is how ADR 0002 gets
  violated by omission rather than by argument.
- **Instances are keyed by `(NodeId, component index)`**, because a node may run several
  scripts, and the update order is scene pre-order - the same walk `collect_sprites` uses.

## Ground truth checked before planning this

- `helios::ScriptHost` has no caller outside its own tests. atlas uses comet only for the
  language service.
- `ScriptState` holds a `Transform` and a list of printed lines, and nothing else. The host
  property accessors end in catch-all arms, so a property added to `PROPERTIES` without its
  matching arm compiles and silently reads zero - the failure that bit three times in 4.x.
- There is no file watcher anywhere and no `notify` dependency. The working precedent is the
  theme's 400 ms mtime poll in `spectrum::settings`.
- atlas has never observed a key *release*: the `KeyboardInput` arm is guarded on
  `ElementState::Pressed`, and `KeyEvent::repeat` is never consulted. A game asks "is left
  held now", which is a state question the application currently cannot answer.
- aurora's `InputEvent` has no release event at all and `Key` is 22 text-editing verbs, so
  game input cannot route through aurora's focus model. It is a second path from winit.

## Scope

**In:** a Camera component; an input state a script can read; a runtime that owns instances
and steps them; play/stop with a scene snapshot; an execution budget; script output and
traps reaching the editor; an mtime watcher and the reload swap.

**Out, deliberately:** a fixed timestep, pause and step-a-frame, the named action map (v1 is
a fixed set of schema properties), script-driven spawn and despawn, `on_destroy` on node
deletion, the Add Component picker, play-in-a-separate-window, and typing in the Code pane
during Play.

## Ordered steps (each about one commit)

### Part A - make it safe to run anything

1. **An execution budget in helios.** `Engine` with `epoch_interruption`, a deadline set per
   `update` call, and a `stopped` flag on `ScriptInstance` so one bad frame does not produce
   an error every frame forever. The trap already names the comet function; turn it into a
   console line with a source position through `comet::service::function_line`.
2. **Store limits.** `StoreLimits` on the store and a `maximum` on comet's memory, so a
   script that allocates in a loop fails as a script rather than as the editor.

### Part B - voyager becomes real

3. **`voyager::Runtime`**: owns a `ScriptHost`, the `(NodeId, component index) -> ScriptInstance`
   map, an input state, and `step(&mut Scene, dt)`. Fifty lines that tick transforms is
   enough to start; the point is that the boundary exists before there is Play state to move
   out of `atlas::State`. voyager depends on helios; atlas depends on voyager.
4. **Lifecycle**: instantiate on Play in pre-order, `start` once, `update` per frame,
   `destroy` on Stop. Scripts that fail to compile are reported and skipped, not fatal.

### Part C - the editor can press it

5. **A play/stop state in atlas**, with the scene cloned on Play and restored on Stop, and a
   gate on every mutating path - gizmo grabs, the drag apply, delete/duplicate/paste, the
   inspector commits, and the history shortcuts.
6. **Toolbar buttons**, and a viewport that says unmistakably that it is playing: in-process
   Play means the editing UI is still there and still clickable, so the mode has to be
   visible or a drag during Play looks like a loss of work when Stop reverts it.
7. **Call `runtime.step(dt)` in the frame loop.** *This is the demo* - the bounce script
   visibly moves the sprite.
8. **Script output to the console** by draining `take_printed_tagged`, without setting
   `dirty` - a rebuild drops aurora's retained state, and `poll_console` currently sets
   `dirty` on any log change, which during Play would rebuild the shell several times a
   second.
9. **The inspector refreshes in place** during Play through the existing
   `sync_inspector_transform` mechanism, extended past the transform, rather than by
   rebuilding.

### Part D - a game, not an animation

10. **A `Camera` component** and a Play view that comes from the scene, falling back to the
    editor camera when there is none, so Play demoes before anyone has added one.
11. **Input in the host schema**: `input.left/right/up/down/action` as bools and
    `input.mouse` as a Vec2 - rows in `PROPERTIES` and arms in the accessors, nothing in the
    compiler. This needs the winit seam to observe key *releases* first, which it never has.

### Part E - hot reload

12. **An mtime poll** for the project's `.cmt` files, modelled on `poll_settings`.
13. **The swap**: `forget` the module, recompile, `reconcile` the component's values,
    instantiate with them, replace the instance. A source that no longer compiles leaves the
    old instance running and reports - a broken save must not stop the game.

## Testing strategy

The runtime is headless by construction, which is the point of it living in voyager: a
`Runtime` over a `Scene` with a fake host needs no window and no GPU, so steps 1-4 and 12-13
are ordinary tests. Specifically worth pinning:

- a runaway script traps on its deadline rather than hanging, and is not called again;
- Play then Stop leaves the scene's RON byte-identical to what it was before;
- a reload keeps a tuned `@export` value and does not re-run `start`;
- a reload of a source that does not compile leaves the previous instance running;
- update order is scene pre-order, and a node with two scripts runs both.

The parts that need a person are the ones that always do: whether the play mode reads as a
mode, and whether the reload feels immediate.

## Verification

The milestone is done when the demo project's bouncing sprite moves on Play, stops and
reverts on Stop, and editing `bounce.cmt` in the Code pane and saving it changes what the
sprite does without restarting - with the tuned `@export` values still where the inspector
left them.
