# Ideatank

The idea inbox: anything worth doing that we are not doing right now lands
here, one bullet per idea, grouped by area. No commitment implied - an entry
is a captured thought, not a promise.

**The ritual:** once a milestone is basically done, an open-ended iteration
phase begins: we go through this list together, pick a bundle, implement it,
and repeat for as long as we want. Iteration work is not part of the
milestone itself. When an idea ships, delete its entry (git history
remembers); when one dies, delete it too.

Seeded after M3 (2026-07-23), swept on 2026-07-28, swept again on 2026-07-30
after the six iteration-phase reports (3.1-3.6) were written - the writing
itself was an audit - swept on 2026-07-31 when M4 shipped, which retired the
whole "Toward Comet" section by building all six of its entries, and swept on
2026-08-03 after the whole-project review
([review-2026-08-03.md](review-2026-08-03.md)). Swept again the same evening
once M5 shipped, which is why several sections below are new: Play exists, so
the questions a running game asks are real for the first time.

## Where the project actually stands

Worth stating plainly, because it should steer what we pick next.

Line counts, all `.rs` under each crate including tests: atlas 19949, comet
16271, aurora 13094, helios 4692, aurora-wgpu 2412, photon 1721, voyager 1288,
prism 846, spectrum 537, sandbox 350, aether 110. The editor and the GUI
framework are still the mature part; `comet` is a complete language through
containers and annotations; and `voyager` stopped being a stub in M5 - it is
now the crate that owns the loop, the instances, the input and the reload.

**M5 changed the shape of the question.** The scene is no longer a pile of
static sprites: scripts run in the viewport, a Camera component decides what
the running game sees, five keys and a mouse reach a script, and saving a
`.cmt` swaps it in under a live game. What that bought is a first game. What
it exposed is how far off a *second* one is:

- **A script can move its own node and nothing else.** The host surface is
  `transform.*` and `input.*`. No script can see another node, spawn one,
  remove one, or touch any component other than the transform - including the
  Camera M5 itself added. A bullet, a pickup, an enemy wave, a score that goes
  up when two things touch: none of it can be written at all.
- **There is no clock and no randomness**, so a cooldown needs a hand-rolled
  accumulator and nothing can vary between runs.
- **The demo project cannot demonstrate the milestone.** Nothing in
  `demo_project` reads `input`, no scene has a Camera, and the scene the editor
  opens runs a scratch file rather than any of the seven annotated teaching
  scripts. A first launch after the killer feature shipped shows three squares.
- **helios has three components now** - Sprite, Script, Camera - and the third
  one proved the model. The next ones are blocked on `Value`, which cannot hold
  a list, so AnimatedSprite's frame list has no representation.
- **photon still draws exactly one thing: textured quads.** The grid, axes and
  gizmos are faked with sprites; there are no line, circle or arc primitives
  and no world-space text.

So the choice the next bundles face has flipped. It is no longer "polish the
authoring experience or give the engine something worth authoring" - M5 gave it
something. It is now: **make the one game you can write actually writable**
(node access, a clock, spawning), or **make the thing that exists teach**
(a demo that shows the milestone, a manual, a way to see a frame). The two are
closer together than they look, because the demo cannot be written until the
language can express it.

## Play, and the runtime

New in M5, so this whole section is new. The runtime is 1288 lines and it is
now the thinnest part of the project by a wide margin.

- **Nothing anywhere says a script has stopped.** A trapped instance latches
  itself off, `ScriptInstance::is_stopped` and `resume` exist for exactly this,
  and neither has a caller outside helios's own tests. So a script that divides
  by zero stops moving its node, the game keeps running, and the only evidence
  is one console line that arrives up to 500ms later. A `Runtime` accessor
  listing what is running - node, source, running/stopped/failed, last error -
  is small, and it is the single most educational artifact Play could have: it
  turns the execution budget from a mystery into a visible rule. A Running pane
  is also where pause, step, per-instance restart and a per-script cost readout
  would all hang.
- **Play's errors arrive as strings and lose everything.** helios goes to real
  trouble here - `Compile` carries every `Diagnostic` with spans, `Trapped`
  carries the comet function lifted out of the wasm backtrace, and its doc says
  why ("an editor can act on this one: it has somewhere to put the caret").
  voyager flattens both with `format!("[{}] {err}")` into a `Vec<String>` and
  atlas re-emits the string through tracing. So the errors from actually
  running the game are the only errors in the editor that cannot open a file,
  cannot squiggle a line, and never reach Problems. A typed `Problem { path,
  function, line, diagnostics, message }` that still `Display`s as today's
  string is the fix, and `service::function_line` already turns a function name
  into a line - its only caller in the workspace is a helios test.
- **No restart, and restart is the button you press most.** F5 stops, F5
  starts; two presses and two full shell rebuilds for the most frequent action
  in any edit-run loop, with no other path. Shift+F5 as stop-then-start in one
  call is small, `Runtime` already pins the semantics in a test, and it is the
  one escape hatch a trapped script currently lacks.
- **A script broken when Play is pressed can never be fixed while the game
  runs.** `bring_up` records the mtime only after a successful compile, so a
  file that does not compile at Play is never watched, and `reload` scans
  `running`, which by construction never holds a failed instance. The
  beginner's loop is exactly this - press Play, read the error, fix the typo,
  ctrl+S - and nothing happens. Note this reverses a decision recorded in a
  comment, so it is a request to revisit rather than a bug report.
- **Tune it while it runs.** `apply_exports` already writes a module's globals
  and is called once, at instantiation; `read_exports` reads them back and the
  inspector shows them live. What is missing is a public write to a live
  instance. Un-gating exported rows specifically does not weaken the twenty-six
  other gates, because ADR 0022 keeps the component the owner - the authored
  value is untouched and Stop still has nothing to restore. Drag `gravity`,
  watch the jump change: it is what `@range` and `@step` were built for, and
  the obvious first demo for any teaching material about Play.
- **The budget is per call, not per frame.** `FRAME_BUDGET` is armed before
  every `update` and `step` loops instances with no clock of its own, so twenty
  runaway scripts cost twenty times 100ms - measured at 2.01s for one `step`.
  The trap latches, so it is a cliff at Play rather than a permanent cost; the
  other half has no floor at all, since twenty merely-slow scripts at 50ms
  never trap and cost a second every frame forever. A deadline taken at the top
  of `step` covers both. Every existing budget test uses a single instance.
- **A fixed timestep inside `Runtime::step`.** `step` takes whatever `dt` the
  caller hands it, and atlas hands it the editor's frame time clamped to 100ms.
  So jump height changes with refresh rate, a 100ms hitch advances a character
  100ms in one step, and every test hand-steps 0.016 - which means the loop the
  tests pin is not the loop the editor runs. An accumulator, a cap on catch-up
  steps, and a tick count returned from `step` is about thirty lines and moves
  the decision to where a shipped game inherits it.
- **A reconcile pass at the top of `step`.** `play` builds the instance list
  once and nothing ever looks at the scene's shape again. Hidden today only
  because nothing can add or remove a node mid-game - but a node deleted during
  Play would keep running, since `Scene`'s delete detaches rather than removes,
  so the id stays valid and the instance writes into a node nobody draws. One
  pass that attaches new script components and destroys instances whose node is
  detached is about forty lines and no language work.
- **Suppress the editor's overlays during Play.** The scene is drawn through
  the game camera and then the grid, axes, selection outline and gizmo handles
  are appended unconditionally - all built in world space from the *editor*
  camera, down to the line thickness. The moment a scene has a Camera the two
  disagree: a finite patch of grid at the wrong thickness floating over the
  game. Two lines; noticing it was the whole cost.
- **The Play gate refuses selecting, which is not an edit.** `viewport_press`
  returns at the gate before it does anything, so during Play you cannot click
  the sprite that is moving - while the same selection change from the scene
  tree is not gated at all. The live inspector readout follows the primary
  selection, so the readout M5 built is aimed at whatever you had selected
  before you pressed F5, and the gesture for re-aiming it is the one refused.
  Split it: pick allowed, gizmo grab refused.
- **Play does not take the keyboard.** A game only sees a key when nothing is
  focused, so the natural loop - edit a script, ctrl+S, F5 - starts a game that
  ignores WASD, and worse, pressing W types a `w` into the script you are
  running. Dropping focus in `toggle_play` is one line next to the existing
  end-drag run; saying so on the viewport frame that already turns orange is
  the half that teaches.
- **A play clock and a time scale.** The game runs on the editor's frame time
  and nothing says so - no elapsed play time, no frame count, no sign when a
  400ms file open handed the game a step it did not earn. Multiplying `dt`
  before `step` gives slow motion and fast forward for about ten lines, and
  slow motion is how you *see* a physics bug rather than infer it. A pause is
  the same flag at zero.
- **`input.mouse` is wrong in two ways.** It is fed unconditionally every frame
  without asking whether the pointer is over the viewport, so moving the mouse
  to the Inspector sails a mouse-following script off the level. And the script
  is handed its node's *local* transform while the mouse is world-space, though
  `input.rs` promises they are comparable - true only while every scripted node
  is a direct child of an unmoved root, which is exactly what the demo scene
  is. Either add a world accessor beside the local one or narrow the promise,
  but not neither: this is the class of bug a learner concludes is their own.
- **The game's screen is whatever the dock left over.** `CameraComponent::view`
  derives the visible world from the viewport widget's pixel size, re-measured
  every frame from the dock layout, so dragging a splitter while playing shows
  the player more of the level. The roadmap's north star is a *one-screen*
  platformer and "one screen" has no definition anywhere in the codebase. A
  game resolution plus letterboxing is what makes a layout mean the same thing
  twice.
- **There is no second scene, and no way to get to one.** `Runtime` never owns
  a scene - `step` takes one, and instances key on `NodeId`s with nothing
  stamping which scene they came from, so handing `step` a different scene
  resolves keys against the wrong nodes. Mechanically the swap is nearly free
  (`stop` then `play`); what does not exist is anything that survives it, since
  every instance's state lives in a store `stop` drops. That is the same
  question save games ask.
- **Per-script frame cost, reported by the runtime.** `step` times nothing, so
  "which of my scripts is slow" has no answer. Measured at 500 nodes running
  the plan's player script: 0.18ms per frame total, 4.9MB of RSS across 500
  wasmtime stores, 9.3ms to Play and 9.1ms to reload all 500 - so the aggregate
  is not the interesting number and the runtime is not what breaks at 500. The
  distribution is what a person needs, and nothing collects it.
- **The bindings a game plays by live in the editor.** `BINDINGS` and
  `GameKeys` are in atlas, encoding three rules the comments argue for at
  length. voyager depends on helios and nothing else, so the shipping binary
  must either build all of it again or change voyager's dependency shape - and
  a duplicate is exactly how a game and its editor come to disagree about what
  jump is.
- **Decide the shipping binary's dependency shape before M6 needs it.**
  `voyager/src/main.rs` is five lines printing "nothing to play yet", and a bin
  shares its package's `[dependencies]` - so giving it a window makes photon,
  winit and aether dependencies of the library the editor links, which is the
  exact separation the manifest comment argues for and the thing that makes
  voyager's 31 headless tests possible. Optional deps with `required-features`,
  or a separate player crate. A fifteen-minute decision now, a refactor of the
  shipping target if taken during M6.
- **Audio has no home, and the crate docs already claim it.** helios describes
  itself as "scene tree, input, audio, physics, script host"; audio appears
  nowhere. The roadmap defers it to M7+ with kira named, but the shape is
  already shipped and proven: `print` reaches the editor as a per-instance
  queue drained by the frame, and a sound request is the same pattern. A
  one-screen platformer with no jump sound does not read as a game, and "why
  does my sound arrive a frame late" is a genuinely good lesson.

## Comet: what a script still cannot say

M4 finished the language and 4.1-4.9 completed it; M5 made scripts *run*, which
is a different test. These are what writing a real game hits first, each
grounded in what the checker reports today.

- **`if` as an expression, and a block as a value.** `if` is a statement, and
  that one gap produces three failures that look unrelated: `let dir = if left
  { -1.0 } else { 1.0 };` is three errors; a match arm written as a block is an
  eight-error cascade led by a false "this `match` does not cover `Run`"; and a
  function whose body ends in an if/else is told "some paths reach the end
  without" returning, which blames the author for something they did write.
  Codegen already has the lowering - `emit_arms` branches into a reserved local
  region and reads the result back - so an if-expression is that with a bool
  test. This removes two of comet's three worst diagnostics at once.
- **A script cannot see another node, make one, or remove one.** The largest
  single thing standing between helios and an engine somebody finishes a game
  on. The pieces are half there: `Runtime` keys instances by (node, component),
  `Edit::Link`/`Unlink` already express add and remove undoably, and `update`
  is handed the whole `Scene`. What is missing is a `Node` type in the language
  and a way for a running script to ask for a structural change - the host
  functions get a `Caller<ScriptState>` with no scene in it, so the shape is a
  deferred command queue drained between frames rather than a direct call. The
  M5 plan listed three candidate designs (a name lookup, a stable node path, an
  exported node reference) and the language design record already writes `enum
  Hit { Miss, Wall(f32), Node(Node) }` as an example, naming a type that does
  not exist. Spawn and despawn are the same handle problem from the other end
  and should be decided together.
- **A script can only touch a Transform, so a Camera nobody can script is half
  a Camera.** `ScriptState` is a `Transform`, an `Input`, a print buffer and a
  limiter. So a script cannot flash its sprite red, resize it, swap its
  texture, read its own node's name, or turn a camera on - `CameraComponent`'s
  `active` and `zoom` are both types the schema can already express and neither
  is reachable. Decision 2 of the language design record says helios generates
  the schema "from the `Reflect` contract it already has"; `build_schema` walks
  a hand-written const table and `Reflect` is never consulted. Making that true
  is what turns reflection into a fourth consumer of one contract, and it is
  the change that doubles the value of every future component kind.
- **No clock and no randomness.** No elapsed time, no frame count, so "three
  seconds after start" needs a hand-rolled accumulator; and no `random()`, so a
  spawner, a shuffle, or variation in an enemy's speed cannot be written. The
  two differ in cost: `time.elapsed` is one row in `PROPERTIES` and touches no
  compiler, which is the whole point of ADR 0020, while `random()` is a builtin
  and therefore comet as well. Seeding it per session is itself a teaching
  artifact - the same seed replays the same game.
- **`==` on payload-free enums, on Vec2, and on String.** The checker refuses
  equality for everything except numbers and `bool`, so `if mode == Mode::Run`
  is an error and the idiomatic state machine has to be a `match` whose arms
  are all `true` and `false`. It does not have to be this way: codegen lays a
  payload-free enum out as exactly one i32 and already emits `i32.eq`, so the
  emitted code would be correct today and the checker guard is the only thing
  in the way. Vec2 is two comparisons and an `and`. Only String is real work,
  which is why the original comment refused all three together.
- **The loop surface: no `break`, no `continue`, no `for x in a`.** `break`
  lexes as an identifier, so writing one reports "cannot find `break` in this
  scope" - a message that tells a learner they misspelled a variable. `for` is
  a counted range only, so walking the platforms and stopping at the first one
  you are standing on is an index loop carrying a `found` flag, and the flag is
  the part that teaches the wrong lesson. The machinery is in place at both
  ends: the `while` lowering already emits the `block`/`loop` pair a `br` would
  target, and the checker already desugars `for i in a..b` into that shape.
- **`str` only knows numbers, and `print` is the only thing a script can say.**
  M5 wired `print` to the console and thereby made it the whole of a script's
  observability - and `print("grounded: " + str(grounded))`, the first debug
  line anyone writes, is "expected `f32`, found `bool`". The workaround for a
  bool needs a helper function with an early return, because `if` is not an
  expression either. `str(bool)` and `str(Vec2)` are a host variant and a
  binding; `str(enum)` is a different job, since the variant names live in the
  compiler and it lowers to a branch over string literals.
- **The schema cannot say read-only, so `input.jump = true` compiles and does
  nothing.** Every write to an input property is dropped by a match whose arms
  are all empty, and the code names it as a defect in its own comment. For an
  educational engine this is the worst shape a gap can take: no diagnostic, no
  effect, no way to find out why. One bool per schema row and one check in the
  place-resolution path, and the value goes up the moment the schema grows -
  most of what a script will ever reach (the mouse, elapsed time, a parent's
  position) is read-only.
- **A function cannot change a struct it is given, and nothing says so.**
  Structs are values and arrays are references, a distinction the demo scripts
  teach carefully - but the place it bites has no diagnostic at all. `func
  hurt(body: Body, amount: f32) { body.hp -= amount; }` compiles clean and
  leaves the caller's struct untouched. After the 4.x sweep this is the last
  silent do-nothing left in the language, and it is the entry point to the
  still-open "which warnings does comet emit" question with a case that
  matters.
- **A hot reload re-runs every top-level `let` but not `start`.** M5 decided a
  reload does not re-run `start`, because a `start` that places its node would
  teleport it on every save - but instantiating the new module still runs the
  wasm start function, which is where top-level initializers evaluate. So the
  two init sites have opposite reload semantics, and `bounce.cmt` is written
  across both: `let velocity = ...` re-runs and `func start() { velocity = ...
  }` does not, so a reload leaves the sprite stopped. That is the milestone's
  own verification case producing a broken sprite. The fix is language-shaped -
  `start` becomes the only init site, or a `let` can say "keep what you had",
  or there is an `on_reload` - and `bounce.cmt` has to be rewritten to match.
- **Generics stop at the enum declaration.** Decision 10 is recorded as
  implemented, and what shipped is type parameters on *enum* declarations only:
  `func first<T>(a: Array<T>) -> Option<T>` fails at the parser, as does
  `struct Pair<T>`. So `Option<T>` and `Array<T>` are generic and a script's
  own helpers cannot be - the first utility anyone writes over an array has to
  be written once per element type. Worth either building or writing down as a
  deliberate v1 boundary, because the doc claims more than the parser delivers.
- **Three of the seven annotations describe values a script cannot declare.**
  `@color`, `@asset` and `@multiline` are validated by the checker and cannot
  be attached to anything, because `@export` accepts no `String` and there is
  no four-component type. Of the seven hints atlas reads only three. The engine
  side is already built - `Value` has `Str`, `Color` and `Asset` and the
  inspector draws all three for a Sprite - so comet is the missing piece, and
  `@asset` is the one that matters: it is how a script picks its own texture.

## Comet: what it says when you are wrong

Orbit is an educational engine, so the quality of a wrong-code message is a
first-class feature rather than polish.

- **The language service throws the checked tree away.** `Analysis` keeps
  diagnostics, tokens and brackets and drops both the AST and the `TypedScript`
  the check produced, so every type-aware question - hover, definition,
  completions, signature help - re-runs the frontend over raw text, and
  `type_of_name` runs a whole second `check()` to answer one hover. The cost is
  not the milliseconds; it is that every type-aware answer the editor now wants
  is blocked on a tree the service already computes and discards. Making
  `Analysis` own them is the one change that makes most of this section cheap.
  (code-editor-backlog.md marks this DONE - only the caching half shipped, and
  that mark should be corrected rather than the entry written twice.)
- **A line table: the last mile from a trap to a caret.** `function_line` maps
  a trapping function to the line it is *declared* on, and its own doc says why
  that is the best it can do: a wasm trap names a function, not a line. Its
  only caller in the workspace is a helios test. The APIs for the missing piece
  exist - `Function::byte_len()` during emission, `FrameInfo::func_offset()` at
  the trap - but `TypedStmt` carries no span on any arm, so a per-statement
  table means putting spans on TIR statements first. That is what makes the
  size honest, and it is the prerequisite for runtime squiggles, breakpoints
  and step-a-frame.
- **Every `unreachable` comet emits should have a sentence.** M5 built exactly
  the right machinery and used it twice, for the frame budget and the memory
  cap. But `comet_array_at` traps with two bare `unreachable`s, so reading past
  the end of an array tells a learner "wasm trap: unreachable (in `update`)" -
  and helios has a test asserting that message is kept. Stack overflow falls
  through the same arm, and unbounded recursion is something someone hits in
  the first hour. One more discriminator and one arm for `StackOverflow`, plus
  two sentences.
- **Six diagnostics say `enum`, `struct` or `Array` instead of the name that
  was written.** `name_of` exists precisely to stop "expected `enum`, found
  `enum`", and six sites still call `Type::name()` raw - so a real script sees
  "`match` works on an enum, and this is a struct" and "a `Array` cannot be
  exported yet" (also the wrong article). Every script that hits one of these
  has several enums or structs in it and the message names none of them.
- **The checker knows two spans and reports one.** An unclosed `{` is reported
  at the token where the parser gave up, so the commonest structural mistake in
  the language squiggles an innocent later declaration - while the opening span
  is sitting right there as `block_inner`'s own `start` parameter. The same
  one-eyed shape repeats in the checker: "already defined in this script"
  points at the redefinition and never at the definition. A `related:
  Vec<(Span, String)>` on `Diagnostic` needs no codes or fix-its to be worth
  having.
- **The service still speaks the pre-M4 language.** `get(a, i)` - the only
  array read that does not trap - is absent from the service's `BUILTINS`, so
  it never completes, never hovers, never shows signature help, while `a[i]`,
  the form that traps, is one keystroke and fully discoverable. The test that
  exists to stop this drift iterates the service's list rather than the
  checker's, so it cannot see a name the checker gained. Field completion
  answers only for `Vec2` and host objects, and hover prints `Type::name()`, so
  a user enum reads "mode: enum".
- **Hover has two answers that are simply false.** The service hands out `f32`
  for a `for` counter with a comment asserting it, while the checker types it
  `int` - and the service's own go-to-definition test is written against a loop
  that does not compile. And `hover_at` has no member-position guard even
  though `definition_at` grew one and explains why, so with a script-state `let
  x = true;` at the top, hovering the `x` of `transform.position.x` answers `x:
  bool`. Both are one-line fixes. They matter more than their size because
  hover exists specifically to teach types, and a teaching tool that is
  confidently wrong is worse than one that says nothing.
- **"Never called" should mean unreachable from a hook, and should guess.** The
  unused-function warning fires when no call site names a function, which
  answers the wrong question now that the engine is what starts things: in
  `func a() { b(); } func b() { }` with no `update`, `a` is warned and `b` is
  not, though neither can run. Reachability from the hooks table is a fixed
  point over call sets the checker already collects. And the warning never
  guesses, though `nearest` is in the same file and every other name error
  already says "did you mean" - `func updat(dt: f32)` should read "did you mean
  `update`, which the engine calls every frame?".

## Engine (helios)

- **A node has no identity that survives a save.** `NodeId` is a slotmap key
  and `NodeDoc` has no id field, so a node's only address on disk is its
  position in the tree. This is already load-bearing in a way nobody chose:
  Play's snapshot has to be a `Scene::clone` rather than a RON round trip
  precisely because a round trip renumbers everything the editor holds outside
  the scene. Two live answers - a `u64` per node, which survives renames and
  reorders but costs a migration, or a stable path re-bound by name, which
  costs nothing on disk and breaks on a rename. Nothing today can point at a
  node across a save, and the price of deciding rises with every scene file
  written. It gates node-reference fields, instancing overrides, script-side
  find, and multi-scene addressing.
- **`Value` cannot hold a list or name a node.** The closed set is
  F32/Int/Bool/Str/Vec2/Color/Asset, so AnimatedSprite's frame list - the next
  component this file asks for - has no representation, and neither do
  waypoints or a tilemap's cells. No component can say "follow this" or "spawn
  that" either. Adding either is a deliberate change to ADR 0016's closed set
  plus an inspector row that can show it. (Not to be confused with
  `Str`/`Color`/`Asset` falling through `apply_exports`: no script can reach
  that path, because comet refuses to export a String outright until somebody
  writes the ownership rule.)
- **More components.** Camera shipped in M5 and proved the model - one enum arm
  plus one `Reflect` impl. The remaining candidates now have a shared blocker
  rather than a shared cost: **AnimatedSprite** (sprite sheet + frame list +
  fps) needs a list in `Value`, **Text** needs world-space text in photon, and
  **Tilemap** needs both. The cheapest next one is whatever the north-star
  platformer needs, and the Add Component picker becomes worth building the
  moment there are four.
- **Hiding a node does not stop its script.** `Node.visible` is documented as
  render-only and `Runtime::step` never looks at it, so an enemy you switch off
  in the tree keeps moving, invisibly. There is no per-component enable either.
  The code is one bool plus one check; the decision is what needs writing down
  - does `visible` mean "not drawn" or "not there", and if they stay separate,
  does the tree show both. It is also what object pooling and per-script pause
  would use.
- **The scene format has no version, and Save quietly relocates the file it
  loaded.** Nothing carries a format version, and an unknown component kind
  fails the whole load three lines above a comment promising forward
  compatibility for unknown *fields* - so a scene from a newer build survives a
  new field and dies on a new component. Worse, `Project` does not remember
  where its scene came from: `load` honours `manifest.main_scene` but `save`
  always writes `scenes/main.ron`, so opening a project whose scene lives
  elsewhere and pressing Save orphans the original. Latent only because the one
  project atlas can open already says `scenes/main.ron`.
- **Saving a script edits the scene behind undo's back.** When a `.cmt` file's
  `@export` set changes, atlas reconciles the affected components by writing
  straight through `node_mut` and then marks the project dirty. It has no
  choice - `Edit` has no variant that can express "this component's declared
  field set changed". M5 made this reachable twice over, on every ctrl+S and on
  every hot-reload poll, so renaming an export now mutates the scene in a way
  undo cannot reverse. Either `Edit` grows a `SetExports`, or the rule becomes
  "reconcile is not an edit" and it stops dirtying the project.
- **Move the migration walk down into helios**, where ADR 0008 says it belongs
  - it currently lives in atlas as `reconcile_script_exports`, and voyager's
  reload path has to trust that atlas called it first.

## Renderer (photon)

- **The pick id is the array index, and that blocks three entries below.**
  Picking numbers sprites by where they sit in the submitted buffer, and
  `build_runs` packs one buffer for both the drawing pass and the pick pass
  precisely so the id maps to the flattened sprite index - its own comment says
  so. That is exactly where culling, a z-sort, or an atlas merge would want to
  live, and any of them silently selects the wrong node the moment it does. An
  explicit `id: u32` on `Sprite` costs four bytes per sprite and two lines of
  pick.wgsl. Do it before the three, not after.
- **photon shape primitives**: lines, rects, circles, arcs, polygons. Still the
  highest-leverage single addition - one change retires the gizmo arrowheads,
  the rotate feedback arc and a crisper grid, and gives games and debug-draw a
  real API instead of quad tricks.
- **World-space text** (photon reusing aurora's glyph atlas machinery) - node
  labels in the viewport, debug overlays, in-game text later.
- **Seven draw paths, and every new option costs seven edits.** render,
  render_to_image, render_to_target, render_runs_to_target, overlay_to_target,
  pick and pick_runs each build their own encoder and submit; pick and
  pick_runs are near-copies differing only in the middle. A blend mode, a
  sample count, a nearest sampler or a second primitive has to be threaded
  through all of them. One pass builder collapses them and lets the editor's
  scene and overlay share a pass instead of two submits a frame.
- **One sampler for the whole engine: no nearest, no mips, no repeat.** Linear
  everywhere, no mipmap filter, ClampToEdge, and every texture uploaded with
  one mip level - so pixel art cannot be drawn crisply at any zoom, a minified
  sprite crawls, and a tiling background cannot repeat. Minification is not
  hypothetical: the sprite default is a 100x100 quad and the demo ships a
  1056x740 photo. aurora-wgpu already solved this one crate over for icons; the
  renderer should not have worse filtering than the GUI toolkit borrowing its
  device.
- **`Camera` has no inverse, so two other crates wrote one each.** atlas has
  `EditorCamera::screen_to_world` and, since M5, `play_cursor_world`, which
  inverts a real `photon::Camera` inline - that second one is literally the
  missing method, signature and all, including the divide by target size.
  renderer.md claims the Y-down flip lives in exactly one place; its inverse
  now lives in two others, in another crate.
- **A `Sprite` cannot express a shear, and helios documents the workaround.** A
  rotated child under a non-uniformly-scaled parent has no exact `Sprite`, so
  `build_sprite` decomposes the world affine and drops the shear - and the
  gizmo decomposes the same way, so what you see and what you grab are wrong
  together. The GPU was never the obstacle: `RawInstance` already carries a
  full mat3x2. One `shear` field makes `Sprite` fully general, and a
  `from_affine` alone cannot work because there is nowhere to put the sixth
  number.
- **Nothing is culled, and nothing can be counted.** `sprite_draws` emits every
  sprite regardless of the camera, so panning away from the content costs the
  same. A world-AABB test is a few lines and it is also the cheapest teaching
  demo in the engine - watch the count fall as you pan. The renderer panel this
  file already wants has nothing to show and no way to ask, because the drawing
  calls return `()`; a small `FrameStats { instances, draws, texture_switches,
  culled }` is the missing data source. Needs the explicit sprite id first.
- **The per-frame allocation is not only the instance buffer.** The camera bind
  group creates a fresh uniform buffer *and* bind group every call, and
  `build_runs` creates a texture bind group per run per call - so an editor
  frame allocates two uniform buffers, two camera bind groups, one per texture
  run, and two instance buffers, with a click adding a whole R32Uint target on
  top. Caching by texture and reusing one camera uniform are small and
  independent of the persistent-buffer work. Separately, the 56-byte instance
  record benchmarks blame for the throughput collapse past 10k sprites goes to
  36 by storing uv_rect as Unorm16x4 and tint as Unorm8x4, at the price of
  8-bit tint.
- **The scene target is 8-bit linear and single-sampled.** Linear is right for
  a compositing intermediate; eight bits of it bands in the dark theme. The
  sample count is the bigger one - every rotated sprite edge, every 1px grid
  line and every gizmo shaft is hard-aliased and crawls under a pan, while
  aurora anti-aliases its own rounded rects on the same device. A 4x
  multisampled scene target needs a resolve texture, a pipeline cache keyed by
  sample count (which renderer.md already lists as missing), and
  `render_to_image` left at 1x so the pixel tests keep their exact readback.
  The pick pass stays at 1x: ids must never be resolved.
- **aether never installs an uncaptured-error handler.** wgpu's default handler
  panics with a bare "wgpu error: ...". What a handler buys is smaller than a
  rescue and still worth having: a validation error or device-side OOM becomes
  a `tracing::error!` with the editor's context instead of a panic string, and
  it is the seam a device-lost path would hang off. Thirty lines in a 97-line
  crate. (While there: photon has two contradictory comments about what wgpu's
  default handler does, and one of them is wrong.)
- **Nothing owns "draw a scene", and M6 is the milestone that needs it.** Every
  piece between a `Scene` and a window lives in atlas: the path-keyed texture
  cache with disk decode and a magenta fallback, the run coalescing, the choice
  of camera, and resizing the target. voyager excludes photon on purpose,
  photon excludes image decoding on purpose, and photon's windowed path has
  exactly one consumer - the sandbox. So those four pieces have no home a
  shipped game can link, and M6 will either move them or write them twice.
  Cheapest to decide now, with one caller to keep working. It also gives a
  headless render-this-scene-to- a-PNG, which is what scene thumbnails and a
  golden-image net both want.
- **Texture atlas / sort-then-batch.** Per-texture batching shipped, but
  cross-texture painter's order still forces a new draw at every texture
  change.
- **Persistent instance buffer**: write instances in place rather than
  rebuilding and re-uploading every frame. The `instance_pack` benchmark
  already measures the cost.
- **Sprite sheet support**: `uv_rect` exists on photon's `Sprite` and is not
  exposed on `SpriteComponent`. Exposing it is small and unlocks
  AnimatedSprite. Flipping needs no new photon field at all - the shader
  interpolates between the uv rect's corners, so an inverted rect already flips
  the sampling; the gap is entirely above photon.

## Teaching surface

Orbit's stated differentiator. M5 changed what is possible here: a script now
runs, so there is something to watch.

- **The demo project is a language tour with no game in it.** The seven scripts
  in `demo_project/scripts` are a careful commented tour of Comet, and the
  scene attaches none of them - it runs `test.cmt`, seventeen uncommented lines
  of sine and cosine at the project root. There is no Camera, and grepping the
  whole project for `input.` returns nothing. So the M5 plan's own definition
  of done ("editing `bounce.cmt` and saving it changes what the sprite does")
  cannot be run against the scene the editor opens, and a first launch after
  the killer feature shipped shows three squares. Wiring the north-star
  one-screen platformer as the demo scene is a couple of hours, and it is the
  cheapest teaching artifact in the project, the only end-to-end test that the
  four M5 features work together, and the anchor any guided tour would need.
- **A wrong-on-purpose file for runtime failures, beside squiggles.cmt.**
  squiggles.cmt is the project's best teaching artifact - wrong on purpose,
  with a comment saying so. M5 built two runtime protections with genuinely
  good messages ("is there a loop with no way out?", "is something being added
  to in a loop?") and both are invisible until somebody trips them by accident.
  A `runaway.cmt` with a commented-out infinite loop, an allocation loop and an
  unbounded recursion makes the safety net a lesson rather than a surprise. In
  a classroom thirty people write `while true` in the first hour, and the
  message they get is their whole first impression of whether this tool is on
  their side.
- **The frame record: what happened this frame, kept.** `Runtime::step` keeps
  nothing except what went wrong. Which scripts ran, in what order, and how
  long each took is recorded nowhere - and the order is a *decided* rule whose
  argument lives in a doc comment no user can ever see. A `FrameRecord` filled
  by `step` is a small struct and no new dependency, and it is the one
  structure four teaching features all want: the frame profiler, per-script
  cost, step-a-frame's display, and the answer to "why does my platform move
  before my player".
- **Watch every top-level `let`, not only the exported ones.** The inspector
  re-reads a running script's `@export`ed variables every frame; ordinary
  script state is invisible - `velocity` and `home` in bounce.cmt, `mood` in
  states.cmt - and those are exactly the variables a learner is confused about.
  Every top-level `let` already has its wasm globals allocated whatever its
  annotation says; one line withholds them from the export section. Exporting
  them all is additive to the ABI and turns "add a print and squint" into
  "watch the number". Honest limit: String, Array, struct and enum state lives
  in the heap rather than in a global, so v1 covers the four scalar shapes and
  must say so.
- **When a script is stopped, say where it was.** helios runs a thread whose
  only job is to advance the epoch every 10ms, and the deadline currently does
  one thing: trap. wasmtime lets a store install an epoch deadline callback
  that inspects the guest and continues, and `WasmBacktrace::capture` works
  inside it - so the callback can see where the guest is and trap at the same
  place with the protection unchanged. Read honestly this is not a sampling
  profiler: epochs advance on wall time, so a cheap script's samples all land
  at the top of `update`. It is a truthful instrument exactly where it matters
  - "ran for more than 100ms and was stopped" becomes "stopped after 100ms, in
  `wobble`".
- **Show what the game changed - the snapshot is already sitting there.** Play
  clones the authored scene on the way in and uses that clone for nothing until
  Stop. A diff of authored against live, per node and per field, answers the
  question a learner asks constantly: what is my script actually doing to the
  world? It teaches the data model directly, because the answer is that the
  game and the document are the same tree. It also makes Stop's revert
  trustworthy rather than alarming, and it is what would let M5's open question
  be answered per node instead of globally.
- **Record `dt` and input, and replay a run exactly.** Both seams already point
  the right way: `step` takes its dt from the caller, input is *pushed* in
  rather than pulled from a device, and Play already clones the authored scene
  - and there is no clock or random in the schema, so there is no third input
  to record. It buys four things: a bug that reproduces, a lesson that runs
  identically for every student, a headless regression test ("this game still
  plays the same after the renderer change"), and an honest answer to "did it
  behave differently because of my code or because my machine was busy". Two
  honest limits: the epoch budget is wall-clock, and a hot reload mid-recording
  changes the code under the tape.
- **The manual is one sentence, and the tables it should be made of already
  drift.** `docs/manual/index.md` reads "Empty until there is an engine to use"
  - a condition M5 has now met. Orbit's entire user-facing surface is about
  forty rows of const tables: seventeen builtins with signatures, nine
  host-schema rows, three lifecycle names, five input bindings, three component
  kinds, eight annotations. None of it is written anywhere a user can read, and
  the tables have already drifted - `get` is a builtin the checker knows,
  handles specially, offers in its did-you-mean list and the demo teaches by
  name, and it is absent from the table completion and hover are the only
  readers of. Generating the reference from the tables makes that drift
  structurally impossible.
- **In-editor frame profiler.** M1 deferred the on-screen overlay "until Aurora
  + text"; that condition has been met for a while and M5 sharpened it - the
  editor's frame now *contains* a game's frame, since `play.step` runs between
  two existing profiler marks, so the overlay would show the game's cost for
  free rather than needing new instrumentation.
- **A "what is the renderer doing?" panel**: live draw-call count, batch count,
  texture switches, sprites culled vs drawn - and *why* a batch broke. Blocked
  on the drawing calls returning something; see the `FrameStats` entry above.
- **Overdraw / batch visualization** in the viewport: tint sprites by which
  batch they landed in, or heatmap overdraw. Makes an abstract cost visible.
- **Live scene RON view**: a panel showing the serialized form of the current
  selection, updating as you edit. The file format is the data model.
- **Step-a-frame debugging.** Still unbuilt, and now cheap at the runtime end:
  `Runtime` has `playing` and nothing else, and `Play::step` is a single call
  the editor already makes once a frame, so pausing is a flag and stepping is
  calling it once. What it needs to be worth having is somewhere to *look*
  between frames - the frame record and the running-instance list above.
- **Guided tours**: short in-editor walkthroughs that highlight panels and wait
  for the user to act. Expensive, but it is the thing that would make
  "educational" true rather than aspirational. Needs a demo project worth
  touring first.

## Aurora: missing capabilities (framework-level)

Four sweeps have retired most of the original list. What is genuinely still
missing, with the two structural ones first:

- **A `Ui` can be built and thrown away, but never edited.** There is no way to
  remove a widget, and of nineteen public setters only the text ones and
  width/height touch anything layout depends on - no `set_disabled`,
  `set_foreground`, `set_padding`, `set_style`. So any structural or stylistic
  change means building a whole new `Ui` and swapping it, which is what atlas
  does and prism does in miniature. The bill arrives as the ~230 lines after
  the swap that hand-carry retained state back: filter caret, code caret,
  selection anchor, code scroll, reconciled-caret marker, dock sizes, pane
  scrolls, cursor, focus. Every one was a bug before it was a line of code. The
  pressure shows as six independent workarounds - the inspector writing values
  in place during Play, the transform readout during a drag, the drop zone
  appended straight to the draw list, the console refusing to rebuild under a
  held pointer, the tooltip frozen mid-press, and `TabBar` previewing a whole
  reorder as pure draw offsets. Restyle setters are small and mechanical;
  removal plus a real reconcile is the two-week half. It blocks a dirty-driven
  redraw, tooltips and context menus that do not fight a drag, list
  virtualization, and any general animation - all four are currently one
  sentence: "a rebuild would drop the gesture".
- **The focus model is one `Option` that can only hold a text input.** Only
  `TextInput` enters the tab ring, and `activate` is called from exactly one
  place - a pointer release. So a button, checkbox, slider or splitter cannot
  be reached with Tab, cannot be operated with Space or Enter, and never draws
  a focus ring, because there is no focus-ring concept at all. atlas papers
  over it with its own shortcut layer, so what has no shortcut has no keyboard
  path: every visibility eye, every inspector checkbox, every slider, every
  toolbar button. The same single Option is why key routing has no layers -
  atlas invents a four-deep precedence itself, and `ListPopup::key` and
  `FindBar::key` are each called by hand in the right order, though aurora
  already owns a popup stack it hit-tests topmost-first and never routes keys
  through. It is also the accessibility floor: without a focus concept for
  controls there is nothing an AccessKit bridge could expose.
- **Name the pattern the composite widgets already share.** `find`, `list`,
  `picker` and `tabs` are the same shape and each says so in its own header -
  own state, `build(&mut Ui, &Theme) -> SomeRows`, a verdict the app
  dispatches. But each invented its own vocabulary for the verdict and its own
  Rows struct, and nothing codifies the contract, so the fifth starts from
  prose in three unrelated file headers. Codifying it is what lets the select,
  the toggle switch, the stepper, the tooltip and the context menu below be
  written as new files rather than as surgery on ui.rs's 8790 lines.
- **A dropdown/select built on the list popup.** `aurora::list` covers the hard
  half; a closed-set `select` is a small wrapper nobody has written, and it is
  what the Add Component UI and asset-kind pickers want.
- **Tooltips as a framework feature** (popup + hover timer). atlas hand-rolls
  one and has to freeze it while a button is held or it kills a drag.
- **Richer context menus**: submenus, separators, icons, keyboard navigation.
- **Numeric steppers** (+/- arrows on a numeric field), and a **toggle switch**
  as a friendlier boolean than the checkbox.
- **Scrolling has one axis.** `InputEvent::Scroll(f32)` carries a single
  vertical delta, so a trackpad's horizontal gesture is discarded by every
  aurora app. The Code pane has it worse: horizontal text scroll is one field
  on the whole `Ui` and returns 0 for anything not focused, so a `no_wrap`
  editor scrolled 400px right snaps back to column 0 the instant you click the
  scene tree, and no wheel, key or scrollbar can move it sideways at all.
  Widening the event and giving hscroll a per-widget map is small and fixes a
  visible defect; a real two-axis container with a horizontal scrollbar is
  medium.
- **Every pointer move hit-tests the entire widget tree.** `hit_test_node`
  descends into every child without first asking whether the point could be
  inside that subtree, and `layout` calls `update_hover` again every frame. A
  mouse reports at 125-1000Hz while the window presents at 60. A per-widget
  subtree bounding rect built on the return path of `accumulate` - which
  already walks the whole tree every frame - plus an early return is small, and
  it produces exactly the counted-work assertion the quality section keeps
  asking for: widgets visited per hit test.
- **aurora is write-only from the outside.** There is no `children(id)` and no
  iterator over the arena, so nothing outside the crate can walk the tree it
  just built. That one missing accessor blocks three entries in this file at
  once: the layout debug overlay has no tree to outline, the widget gallery has
  nothing to introspect, and a UI regression net has nothing to enumerate. It
  also suggests the headless answer the golden-image entry lacks: `DrawList`
  and `DrawCommand` already derive `PartialEq` and atlas already asserts on
  draw-list commands in three tests, so a golden *draw list* is a text snapshot
  - deterministic, diffable, no GPU.
- **A registered image can never be freed.** Five registration entry points and
  no unregister; `ImageRegistry`'s doc still says Milestone 3 only ever
  registers the viewport, which stopped being true when M4 added icons and
  thumbnails. `Thumbnails` caches by absolute path and never evicts, and
  `ensure_thumbnails` registers one per thumbnailable file in whatever
  directory the explorer shows, with no cap. `free_image` plus an LRU cap,
  before the explorer meets a real asset folder.
- **A winit bridge, because `translate_key` has been written four times** -
  atlas (22 verbs), prism (10), and both aurora-wgpu examples (12 and 6). They
  have already drifted: prism's hex field cannot do ctrl+Home or
  ctrl+Backspace, not because prism decided that but because its copy predates
  those variants. This is the exact test that moved the colour picker into
  aurora, met twice over, and two of the four copies are in the crate that
  teaches people how to drive aurora. aurora-wgpu already dev-depends on winit,
  so an optional feature there keeps aurora itself windowing-free.
- **DPI awareness.** `scale_factor` appears zero times in 61k lines, so the
  whole editor renders tiny on a hidpi display. Already scheduled as part of
  the pre-M6 portability block.
- **An animation story beyond one offset.** `Style::translate` and
  `TabBar::tick` prove the shape; what is missing is anything general - no
  tween or spring type, no way to animate a colour or a size, and every
  animated widget hand-rolls its own decay constant.
- **Icon polish**: node-type icons in the scene tree; disabled state has no
  icon tint. And **disabled state at call sites**: `Style::disabled()` works,
  Save-when-clean still does not use it.
- **Node editor**: a pan/zoom canvas of nodes with draggable ports and wires,
  for a future visual scripting / shader / state-machine graph. A big one; the
  popup, drag-and-drop and splitter groundwork now exists.

## Extraction: still in atlas, but framework-shaped

Milestone 3.6 moved the chrome palette, the colour picker and the tab bar into
aurora, on a rule worth keeping: **aurora grows when a second caller proves the
need, not when the first one suspects it.** Everything below has been written
once, which is exactly why it has not moved - but a second aurora application
would change that overnight.

The boundary that decided the last round still applies: aurora consumes input
and produces draw lists, so anything that touches the filesystem, decodes an
image, or hooks `tracing` stays out however reusable it looks. That rules out `file_ops`,
`explorer`, `console` and the decode half of `thumbnails` permanently.

- **Tooltips**, **the context menu**, and **the modal shell** - all three are
  in the aurora section above as framework features; they are listed here
  because the working implementation already exists in atlas and would be moved
  rather than written.
- **The icon rasterizer.** Coverage predicates, the primitive vocabulary and
  the preview-sheet review loop are general; the icon *set* is half
  editor-specific, and the upload helper reaches for aurora-wgpu. Splitting the
  machine from the art is the work.
- **Property-panel widgets**: a collapsible section card, a labelled row, a
  drag-scrub numeric field, breadcrumbs, a toolbar button. The drag-scrub field
  is the most obviously reusable and the most entangled - it writes through a
  `FieldRef` into a scene and commits through `History`.
- **Docking.** `dock.rs` is already a pure data model, which is why it is
  testable without a UI. Moving it needs it to become generic over the app's
  pane type. The strongest remaining candidate and the largest blast radius.

## Performance

- **Virtualize long lists.** Nothing culls off-screen rows: a scene tree, a
  file listing, or a console with a thousand lines lays out and emits every
  row, every frame. This is the next wall for a big project.
- **A dirty-driven redraw.** atlas still runs `ControlFlow::Poll` with an
  unconditional `request_redraw` and pegs a core on an idle screen; prism uses
  `ControlFlow::Wait` and does not. M5 changed the shape of the fix rather than
  the need: while a game is playing the editor genuinely *is* animating, so
  this is now "wait unless something is running" rather than "wait", and the
  running game becomes the first legitimate reason to keep redrawing.
- **A CI performance gate.** The workable version is not wall-clock but counted
  work: re-shapes, draw-list length, widget count, rebuilds per gesture. Cheap,
  deterministic, and exactly the class of assertion that caught the resize
  regression.
- **Nothing measures the GPU.** Every `timestamp_writes` is `None` - but the
  blocker is not the three call sites: aether requests the device without the
  timestamp-query feature, so enabling it is a device-creation change first and
  a pass change second.
- **Cross-texture batching** still breaks on painter's order, and photon's
  instance buffer is still rebuilt per frame - both detailed in the renderer
  section above.

## Editor: scene editing

- **Add Component picker.** Half its stated blocker is spent: there are three
  component kinds now, and `from_type_name` already builds one from a string.
  It still wants the closed-set `select` control aurora does not have. Worth
  building alongside the fourth component kind rather than before it.
- **A camera you can see and grab.** M5 made the Camera decide what a player
  sees, and the viewport draws no sign of it: `gizmo()` returns `None` for a
  camera-only node and picking goes through `gizmo()`, so a camera cannot be
  clicked, hovered, outlined or dragged - the only way to move it is to type
  numbers in the inspector. Nothing draws the rectangle it will show either,
  though `CameraComponent::view` computes exactly that. It needs a
  fixed-screen- size icon quad to pick against and a world-space frame; both
  are quads and tints, the trick the grid and gizmo overlays already use. The
  same "a node with no extent still needs a handle" machinery covers every
  future component that is not a rectangle - audio emitters, spawn points,
  triggers.
- **Node lock** (excluded from picking and dragging). The visibility eye
  shipped; lock is the remaining half.
- **Sibling order controls**: explicit move-up/move-down. Drag-to-reorder works
  via reparent, but there is no keyboard or button path, and z-order is still
  implicit pre-order.
- **Scene instancing UI.** ADR 0011 settled the design and none of it has an
  interface: no way to instance a scene, see which nodes are instanced, or
  view/revert an overridden value. A designed feature with no front end - and
  it needs the node identity question above answered first, since an override
  has to address a node.

## Editor: viewport

- **Frame-selected (F), frame-all, reset view.** The zoom % readout shipped in
  3.3; the view commands did not.
- **The readouts describe a view the viewport is not using.** During Play the
  viewport is drawn through the scene's camera while the status bar's zoom and
  cursor readouts are always the editor camera's, so the two numbers on screen
  disagree with the picture and with what the script sees - and wheeling moves
  the zoom readout while the image stays put. Once a readout knows which camera
  it is describing, the same answer drives the zoom control and frame-selected.
- **Gizmo arrowheads** and **rotate arc feedback** - both want the triangle and
  arc primitives from photon shapes.
- **Numeric readout near the cursor while dragging** (the last missing piece of
  snapping - the snap math itself shipped and is tested).
- **Checkerboard background** option behind the scene, to communicate
  transparency.
- **Configurable sprite anchor/pivot** on SpriteComponent, centered by default.
- **Mirror/flip** is currently clamped away; the cheap route is the sprite flip
  flags, which need no photon change at all (see the sprite-sheet entry).

## Editor: shell and workflow

- **Nowhere to put the game.** There is no maximize - no F11, no
  double-click-a-tab - so a game plays inside whatever slice of the window the
  Viewport pane occupies, ringed by panels that refuse most clicks while it
  runs. That was fine when the viewport was a scene editor; M5 changed what it
  is for half the time. A maximize is a pure `DockNode` operation on a model
  that is already a testable tree, and `EditorState.dock` is where a maximized
  state would live. The Code pane wants the same thing for a long editing
  session.
- **What the editor forgets between sessions.** `EditorState` persists the
  dock, the camera, the gizmo mode, pane scrolls, collapsed rows and the
  explorer's state - but not which script the Code pane had open, not the caret
  in it, and not the scene selection. M5 sharpened it: the loop is now play,
  quit or crash, restart, go find your script again. The selection needs the
  child-index-path trick collapsed rows already use, so the pattern is written
  and tested. Distinct from "a recovery file is not a save": that is about
  unsaved *content* after a crash, this is about *where you were* after any
  exit.
- **A recovery file is not a save.** The panic hook writes the open buffer and
  the scene to `.orbit/recovered/`, which is a floor rather than a feature:
  nothing offers to restore from it on the next launch, and nothing cleans it
  up.
- **New/Open project**: still hardwired to `demo_project` via
  `env!("CARGO_MANIFEST_DIR")` with no argv path. Part of the pre-M6
  portability block, and a prerequisite for splitting the demo project's three
  jobs below.
- **Scene tabs / multiple open scenes.** The dock handles tabs already; what is
  missing is the model for more than one open scene - and the engine-side half
  of that is `Project` being `{ name, scene }` with a const scene path.
- **Undo-history panel** (the History stack visualized, click to jump).
- **A keyboard shortcut map**: one place defining all bindings, shown in a help
  popup. Shortcuts have sprawled far enough that this is overdue - Add Camera
  (ctrl+shift+K) shipped in M5 with no toolbar button, no menu item and no
  context-menu entry, so it is discoverable only by reading main.rs.
- **Directory watching** in the file explorer, so external changes appear
  without pressing Refresh. The mtime poll M5 built for scripts is the
  precedent.

## Inspector fields

- **Per-field revert to default**, and a visual mark on fields that differ from
  their default. Groundwork for the instance-override UI.
- **Multi-edit**: with several nodes selected, show shared fields and apply an
  edit to all of them. Multi-select shipped; the inspector still shows only the
  primary.
- **A readout format distinct from an editable one.** Live export values reach
  the inspector through `value_to_text`, which formats an f32
  shortest-round-trip - correct for a field that must parse back unchanged,
  unreadable for one that is only being watched, so a driven value churns as
  "83.31999" then "84.147995" sixty times a second.

## Code editor

The Code pane's own backlog lives in
[code-editor-backlog.md](code-editor-backlog.md) - 229 entries from a survey on
2026-07-31, kept separate so one pane's wishlist does not drown this inbox. On
2026-08-01 all twenty defects and the twenty entries judged most important were
implemented; about 190 remain. Read the caution about ordering at the top before
picking from it.

Two of its entries are marked DONE and are not: "One analysis per edit instead
of three whole-file pipeline runs" (only the caching half shipped - the
`Analysis` that carries the AST and TypedScript did not) and "A runtime error
names a line, not a wasm trap" (the comet and helios halves shipped; the atlas
half that routes and draws it did not, and there is still no statement-to-line
table). Both are detailed in the compiler-diagnostics section above; fix the
marks rather than writing them twice.

## Quality and tooling

- **A headless Editor behind the GPU shell.** No test has ever constructed an
  `atlas::State` and none can - it holds a window, a GUI renderer, an engine, a
  texture cache and a scene target. Yet of ~210 methods in `impl State`,
  exactly 13 touch any of those six fields: 26 lines out of 6,357. Everything
  else - the whole press-ordering chain, every shortcut, every history op - is
  pure document logic a test could drive from a tempdir project with synthetic
  presses. atlas has 172 tests and not one mentions `State`, so nothing
  exercises the code that decides what a click means. Splitting the six GPU
  fields into a `Shell` and leaving an `Editor` behind is the single
  highest-leverage test change available, and it is a weekend rather than an
  afternoon.
- **Golden tests for the UI.** Two complementary routes, and the cheap one is
  new: a golden *draw list* is a text snapshot, deterministic and GPU-free, and
  `DrawList` already derives `PartialEq`. The pixel route needs COPY_SRC on
  `SceneTarget`, which is one line - and it would fix three photon tests that
  currently assert nothing: emptying the body of `overlay_to_target` leaves all
  fifteen photon tests green, because the tests for the editor's actual render
  path admit in their own comments that they only prove the call does not
  panic. CI already has a lavapipe job that runs photon's ignored GPU tests.
- **A CI matrix.** Both jobs are `ubuntu-latest`, so nothing has ever been
  compiled on Windows or macOS - and one thing is already broken there:
  `settings_path()` reads `XDG_CONFIG_HOME` else `HOME`, both normally unset on
  Windows, so `save()` returns `Ok(())` having written nothing and the editor
  silently runs on defaults forever. A three-line matrix on the check job is
  what stops the pre-M6 portability block rotting the week after it lands, and
  no leg needs a GPU driver.
- **A test that asserts the dependency graph.** Four manifests carry a prose
  comment about what the crate must not depend on, and all of them are
  checkable: `wgpu::` appears zero times in aurora, helios, comet and spectrum,
  and `std::fs` zero times in aurora and comet. Twenty lines walking `cargo
  metadata` turns the architecture from a comment into a check, which matters
  most exactly when M6 starts adding a window to the shipping path. It reads as
  a teaching artifact too - the layering is one of the few things a learner
  could not work out from the code alone.
- **Split the demo project's three conflicting jobs.** `demo_project` is
  simultaneously the shipped example, the dogfooding scratch space and a test
  fixture, and the three are already fighting: a round-trip test asserts on the
  committed `main.ron` while an editing session rewrites it, and the end-to-end
  Play test deliberately refuses to read that scene, building its own in Rust
  instead, because "it gets dragged around and re-pointed while the editor is
  open". A read-only `examples/platformer/` plus a gitignored scratch project
  (once the editor takes a path argument) lets the fixture stop moving and
  makes `git status` clean while dogfooding.
- **The two saves that are not atomic.** `helios::write_atomic` exists and is
  used for the scene, the manifest and script files. `spectrum::settings::save`
  is not, and it holds the entire authored theme document that prism edits live
  and atlas hot-reloads by mtime - so a crash mid-write leaves a half-file that
  `read()` rejects, and the editor silently runs on defaults with the theme
  gone. `editor_state::save` is the cheap second half. spectrum cannot depend
  on helios, so it needs either eight duplicated lines or the helper moved
  somewhere both can see - which is the same question M6's package writer will
  ask.
- **More counted-work assertions.** atlas now has six `last_measure_count`
  assertions and the picker has one about bitmap re-uploads; the aurora
  hit-test entry above would add the next natural one (widgets visited per
  pointer move).
- **A widget gallery app** showing every aurora widget and style in one place.
  Doubles as a manual, a visual test surface, and the place to try a new widget
  before wiring it into atlas. Blocked on nothing except the arena accessor.
- **Grow prism into a general dev tool.** It is a theming app today; the same
  shell could host the widget gallery, an icon previewer, and a layout
  inspector.
- **An aurora layout debug overlay**: a key that outlines every widget rect
  with its id. Needs the arena accessor.
- **A way to test feel, or an honest admission that there is none.** The
  mechanism is testable, the tuning is not - every interaction adjustment in
  the tab work was found by a person dragging a tab and saying it felt wrong.
  Recorded input traces replayed against the widget tree would at least pin
  *behaviour* under a synthetic pointer, and the headless-Editor entry above is
  what would give them something to replay against.

## Housekeeping

Small, concrete, and none of them are features.

- **ADR 0008 was never amended.** Four M5 commit messages and the M5 plan say
  "a hot reload does not re-run `start`, ADR 0008 is amended to say so". The
  ADR file says nothing about it. The decision is real and enforced by
  `helios::Begin` plus a test; it just is not written where decisions live.
- **The roadmap's M5 entry is still open.** M1 to M4 each carry "(done)" and a
  **Shipped** paragraph; M5 has neither.
- **CONTEXT.md describes two things that do not exist.** photon is defined as
  "sprites, shapes, text, render targets" and draws only textured quads; comet
  is "statically typed, garbage-collected" where ADR 0007 chose refcounting and
  accepted that cycles leak.
- **`demo_project/test.cmt` is scratch work committed at the project root**,
  outside `scripts/`, and it is the file the shipped scene actually runs.
- **No LICENSE file**, and no `license` field in any of the eleven manifests.
  Already named in the pre-M6 portability block.
