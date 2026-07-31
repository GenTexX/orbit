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
itself was an audit - and swept on 2026-07-31 when M4 shipped, which retired
the whole "Toward Comet" section by building all six of its entries.

## Where the project actually stands

Worth stating plainly, because it should steer what we pick next.

The editor and the GUI framework are the mature part of Orbit: atlas is
~13k lines and aurora ~7.5k, against ~1.9k for helios and ~1.6k for photon -
roughly six times as much authoring tool as engine. `comet` and `voyager` are
still one-line stubs. Concretely, that means:

- **helios has two components: `Sprite` and `Script`.** M4 added the second,
  which is what finally makes an Add/Remove Component UI worth building - and
  it arrived with the history edits (`add_component`/`remove_component`) that
  such a UI needs. A third kind is still the thing that would prove the model.
- **photon draws exactly one thing: textured quads.** The editor's grid, axes,
  and gizmos are all faked with sprites (`grid_sprites`, `gizmo_sprites`).
  There are no line, circle, or arc primitives, and no world-space text.
- **There is no input abstraction in the engine**, so nothing can be played
  even in principle. M4 got as far as a script moving a node from a test;
  nothing calls it per frame, which is M5's job.

So the next few bundles have a choice to make: keep polishing the authoring
experience, or start giving the engine something worth authoring. The scene
you can edit so nicely is still, today, a pile of static sprites - but as of
M4 one of them can be handed a script that provably moves it, and the only
missing piece between here and watching that happen is something calling
`update` once a frame.

## Engine (helios / photon)

The thinnest part of the project, and the one that gates the most.

- **More components.** The model supports any number; we have one. The
  candidates, roughly in dependency order: **Camera** (a scene-defined view -
  M5 needs to know which camera Play renders from; today only the editor owns
  a camera), **AnimatedSprite** (sprite sheet + frame list + fps, building on
  `uv_rect`), **Text** (needs world-space text below), **Tilemap** (a big one,
  but it is what makes 2D level-building feel real). Each new variant is one
  enum arm plus one `Reflect` impl - the cheap part - and immediately makes the
  Add/Remove Component UI worth building.
- **photon shape primitives**: lines, rects, circles, arcs, polygons. High
  leverage - one addition retires several entries below at once (gizmo
  arrowheads, the rotate feedback arc, a crisper grid) and gives games and
  debug-draw a real drawing API instead of quad tricks.
- **Sprite sheet support**: `uv_rect` already exists on photon's `Sprite` but is
  not exposed on `SpriteComponent`. Exposing it is small and unlocks
  AnimatedSprite.
- **Sprite flip flags** (flip_h/flip_v) as a cheap alternative to negative
  scale.
- **World-space text** (photon reusing aurora's glyph atlas machinery) - node
  labels in the viewport, debug overlays, in-game text later.
- **Input in helios**: keyboard/mouse/gamepad state, ideally behind a named
  action map rather than raw keycodes. Required before anything can be played;
  worth designing before M5 rather than during it.
- **Texture atlas / sort-then-batch.** Per-texture batching shipped
  (consecutive same-texture runs), but cross-texture painter's order still
  forces a new draw at every texture change. An atlas (one draw) or a
  z-sort-then-batch pass would cut draw calls for interleaved scenes.
- **Persistent instance buffer**: the standing optimization from the renderer
  notes - write instances in place rather than rebuilding and re-uploading
  every frame. The `instance_pack` benchmark already measures the cost.

## Teaching surface

Orbit is described as an *educational* engine, but nothing in the codebase yet
serves that beyond the docs. This is the project's actual differentiator
against Godot or Bevy, and it is currently empty. Ideas that make the engine
explain itself:

- **In-editor frame profiler.** M1 explicitly deferred the on-screen overlay
  "until Aurora + text" - that condition has been met for a while. A panel
  showing the frame breakdown (and the puffin scopes already sprinkled
  throughout) would close a promise the roadmap made.
- **A "what is the renderer doing?" panel**: live draw-call count, batch count,
  texture switches, sprites culled vs drawn - and *why* a batch broke. This
  turns the batching design into something a learner can watch respond to their
  scene, and it is the natural companion to the existing benchmark docs.
- **Overdraw / batch visualization** in the viewport: tint sprites by which
  batch they landed in, or heatmap overdraw. Makes an abstract cost visible.
- **Live scene RON view**: a panel showing the serialized form of the current
  selection, updating as you edit. The file format is the data model; showing
  it teaches both, and doubles as a debugging aid for serialization work.
- **Step-a-frame debugging**: pause the running scene and advance one frame at
  a time. Pairs with M5 Play, and is how you teach what "a frame" is.
- **Guided tours**: short in-editor walkthroughs ("make your first scene") that
  highlight panels and wait for the user to act. Expensive, but it is the thing
  that would make "educational" true rather than aspirational.

## Extraction: still in atlas, but framework-shaped

Milestone 3.6 moved the chrome palette, the colour picker and the tab bar into
aurora, on a rule worth keeping: **aurora grows when a second caller proves the
need, not when the first one suspects it.** The picker moved because it had been
written twice and had started to drift. Everything below has been written once,
which is exactly why it has not moved - but a second aurora application would
change that overnight, and this is the list to reach for when one appears.

The boundary that decided the last round still applies: aurora consumes input and
produces draw lists, so anything that touches the filesystem, decodes an image, or
hooks `tracing` stays out however reusable it looks. That rules out `file_ops`,
`explorer`, `console` and the decode half of `thumbnails` permanently, not just
for now.

- **Tooltips.** atlas hand-rolls a hover timer, a delay, and a popup, and has to
  freeze the whole thing while a button is held or it kills a drag. That freeze
  is framework knowledge living in an app.
- **The context menu**: open at a point, dismiss on an outside press, dispatch an
  action. atlas has it; aurora has only the popup layer it is built on.
- **The modal shell** - a scrim, a centred card, input blocking, Escape and
  backdrop dismissal. Only the shell: `ModalBody`'s settings form and asset
  chooser are atlas's domain and should stay.
- **The icon rasterizer.** Coverage predicates, the primitive vocabulary, and the
  preview-sheet review loop are all general; the icon *set* is half editor-specific
  (gizmo modes, axes, grid, snap), and the upload helper reaches for aurora-wgpu.
  Splitting the machine from the art is the work.
- **Property-panel widgets**: a collapsible section card, a labelled row, a
  drag-scrub numeric field, breadcrumbs, a toolbar button. Every editor has these.
  The drag-scrub field is the most obviously reusable and the most entangled - it
  writes through a `FieldRef` into a scene and commits through `History`.
- **Docking.** `dock.rs` is already a pure data model - a tree of splits and tab
  groups with drop-zone resolution - which is why it is testable without a UI. To
  move it would need to become generic over the app's pane type. The strongest
  remaining candidate and the largest blast radius; the tab bar and the tear-out
  gesture would go with it.

## Performance

Milestone 3.5 fixed what was measured. These are the costs it identified and did
not pay off, plus the instruments it wished existed.

- **Virtualize long lists.** Nothing culls off-screen rows: a scene tree, a file
  listing, or a console with a thousand lines lays out and emits every row, every
  frame, including the ones scrolled out of view. This is the next wall for a big
  project, and it is what would make a panel drag cheap regardless of content.
- **A dirty-driven redraw.** atlas runs `ControlFlow::Poll` with an unconditional
  `request_redraw`, so it re-lays-out and re-draws at full speed on a completely
  idle screen and pegs a core. prism uses `ControlFlow::Wait` and does not. The
  risk is a missed redraw trigger making the UI feel frozen, which is why it has
  not been done casually.
- **A CI performance gate.** `cargo bench --no-run` compiles the benchmarks so
  they cannot rot, but nothing runs them, and timings are machine-dependent. The
  workable version is not wall-clock but counted work: re-shapes, draw-list
  length, widget count, rebuilds per gesture - cheap, deterministic, and exactly
  the class of assertion that would have caught the resize regression.
- **Nothing measures the GPU.** Every `timestamp_writes` is `None`, so all
  profiling is CPU-side and a GPU-bound frame would look free.
- **Cross-texture batching** still breaks on painter's order (see Engine), and
  photon's instance buffer is still rebuilt per frame rather than written in place.

## Code editor

The Code pane's own backlog lives in
[code-editor-backlog.md](code-editor-backlog.md) - 229 entries from a survey on
2026-07-31, kept separate so one pane's wishlist does not drown this inbox. On
2026-08-01 all twenty defects and the twenty entries judged most important were
implemented; about 190 remain. Read the caution about ordering at the top before
picking from it - the sections on running, debugging, and teaching a script
matter more to Orbit than the ones that would make the pane competitive with a
mature IDE, and those are the ones still open.

## Aurora: missing capabilities (framework-level)

Three sweeps have retired most of the original list: scrollbar drag and
track-click, Enter-to-next-row, font weights, text-area scrolling, SDF
anti-aliasing, theming-as-data, ellipsis, the drag ghost and drop highlight;
in the extraction phase a tab bar, a colour picker, and a draw-time offset that
layout ignores (`Style::translate`), the seed of an animation system; and in M4
a monospace face, per-span text colors, decorations, a gutter, find/replace, and
the list popup that was the standing biggest hole. What is genuinely still
missing:

- **A dropdown/select built on the list popup.** `aurora::list` shipped with M4
  and covers the hard half (filtering, keyboard navigation, windowing, caret
  anchoring). A closed-set `select` control - a button that opens it, a chosen
  value, no free text - is a small wrapper nobody has written, and it is what
  the Add Component UI and asset-kind pickers actually want.
- **Tooltips as a framework feature** (popup + hover timer). atlas has a
  hand-rolled file tooltip; that pattern should move into aurora so anything
  can have one.
- **Richer context menus**: submenus, separators, icons, keyboard navigation.
  The popup layer and basic item lists shipped.
- **Numeric steppers** (+/- arrows on a numeric field). The slider and the
  drag-scrub shipped; discrete nudging did not.
- **Toggle switch** as a friendlier boolean than the checkbox.
- **Keyboard scrolling** for scroll containers (PgUp/PgDn/Home/End) - there are
  no key variants for page movement yet. M4 made this sting: a code editor
  without PageUp/PageDown is noticeably worse than one with it, and aurora's
  `Key` enum is where the gap lives.
- **DPI awareness.** Font sizes and metrics are physical-pixel constants and
  nothing reads winit's scale factor; on a hidpi display the whole editor will
  be tiny. This will bite the first time Orbit runs on someone else's laptop.
- **Icon polish**: node-type icons in the scene tree (the file-type icons are
  wired into the explorer now; the tree still shows none). Disabled state has no
  icon tint, though hover and active do.
- **Disabled state at call sites**: `Style::disabled()` works; Save-when-clean
  and Load-when-no-project still do not use it.
- **An animation story beyond one offset.** `Style::translate` and
  `TabBar::tick` prove the shape - a draw-time offset layout ignores, eased per
  frame, with a bool that says "still moving, schedule another frame". What is
  missing is anything general: no tween or spring type, no way to animate a
  colour or a size, and every animated widget hand-rolls its own decay constant.
- **Node editor**: a pan/zoom canvas of nodes with draggable ports and wires,
  for a future visual scripting / shader / state-machine graph. A big one; the
  popup, drag-and-drop, and splitter groundwork now exists.

## Editor: scene editing

- **Add/remove components UI** - an "Add Component" picker and a per-component
  remove button. Blocked on there being more than one component to add (see
  Engine) and on the dropdown widget; unblocks itself the moment both land.
- **Node lock** (excluded from picking and dragging). The visibility eye
  shipped; lock is the remaining half.
- **Sibling order controls**: explicit move-up/move-down. Drag-to-reorder works
  via reparent, but there is no keyboard or button path, and z-order is still
  implicit pre-order.
- **Scene instancing UI.** ADR 0011 settled the design - a Scene embedded as a
  subtree, with per-node value overrides but never structural edits - and none
  of it has an interface: no way to instance a scene, see which nodes are
  instanced, or view/revert an overridden value. A designed feature with no
  front end.

## Editor: viewport

- **Zoom readout and view controls**: a zoom % indicator, frame-selected (F),
  frame-all, reset view.
- **Gizmo arrowheads**: the move handles are plain shafts with square tips.
  Needs the triangle primitive from photon shapes.
- **Rotate arc feedback**: while rotating, draw a pie from the start angle to
  the current one so the amount of rotation reads at a glance. Needs the arc
  primitive.
- **Numeric readout near the cursor while dragging** (the last missing piece of
  snapping - the snap math itself shipped and is tested).
- **Checkerboard background** option behind the scene, to communicate
  transparency. The clear color is themeable; a pattern is not.
- **Mirror/flip via negative scale** is currently clamped away; allowing a flip
  needs care in the world-affine decomposition (or the sprite flip flags above,
  which sidestep it).
- **Configurable sprite anchor/pivot** on SpriteComponent, centered by default
  (ADR 0019 settled the default; the field covers the rest).

## Editor: shell and workflow

- **Confirm-on-quit when dirty.** The unsaved indicator and the modal system
  both shipped; `CloseRequested` still exits immediately, so unsaved work is
  one stray click from gone. Small, and the most likely to actually bite.
- **New/Open project**: still hardwired to demo_project. Needs a native file
  dialog (the rfd crate) or an own popup browser.
- **Scene tabs / multiple open scenes.** The dock handles tabs and rearranging
  already; what is missing is the model for more than one open scene.
- **Undo-history panel** (the History stack visualized, click to jump) -
  History already has the data.
- **A keyboard shortcut map**: one place defining all bindings, shown in a help
  popup. Shortcuts have sprawled far enough that this is now overdue.
- **Directory watching** in the file explorer, so external changes appear
  without pressing Refresh.

## Inspector fields

- **Per-field revert to default**, and a visual mark on fields that differ from
  their default. Groundwork for the instance-override UI above.
- **Multi-edit**: with several nodes selected, show shared fields and apply an
  edit to all of them. Multi-select shipped; the inspector still shows only the
  primary.

## Quality and tooling

Prompted by a perf session on 2026-07-28 that found a resize regression which
had been live for some time behind a test that could not see it.

- **More counted-work assertions.** `Ui::last_measure_count` now counts all
  shaping and a narrowing-drag test guards it; the picker has a test asserting
  that an alpha change re-uploads one bitmap rather than three. Those are the two
  that exist. See Performance for the rest.
- **Golden-image tests for the UI.** photon pixel-tests its output; aurora and
  atlas have no visual regression net, so theming and layout changes can only
  be checked by eye - and several recent ones could not be verified headlessly
  at all.
- **A widget gallery app** showing every aurora widget and style in one place.
  Doubles as a manual, a visual test surface, and the place to try a new widget
  before wiring it into atlas.
- **Grow prism into a general dev tool.** It is a theming app today; the same
  shell could host the widget gallery, an icon previewer, and a layout
  inspector (hover a widget, see its rect, id, and style).
- **An aurora layout debug overlay**: a key that outlines every widget rect with
  its id. Useful for building atlas, and it is also a teaching artifact.
- **A way to test feel, or an honest admission that there is none.** The six
  reports kept arriving at the same division: the mechanism is testable, the
  tuning is not. Every interaction adjustment in the tab work - pin versus ease,
  snap versus glide, the size of a grab band - was found by a person dragging a
  tab and saying it felt wrong. Recorded input traces replayed against the widget
  tree would at least pin *behaviour* under a synthetic pointer, which is more
  than exists today.
