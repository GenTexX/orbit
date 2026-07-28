# Ideatank

The idea inbox: anything worth doing that we are not doing right now lands
here, one bullet per idea, grouped by area. No commitment implied - an entry
is a captured thought, not a promise.

**The ritual:** once a milestone is basically done, an open-ended iteration
phase begins: we go through this list together, pick a bundle, implement it,
and repeat for as long as we want. Iteration work is not part of the
milestone itself. When an idea ships, delete its entry (git history
remembers); when one dies, delete it too.

Seeded after M3 (2026-07-23) and swept on 2026-07-28, when a full audit of
aurora and atlas retired about half the original list. What remains is
grouped below, with three new sections the first pass did not have: the work
that unblocks Comet (M4), the teaching surface the project is named for, and
the tooling that keeps quality from silently regressing.

## Where the project actually stands

Worth stating plainly, because it should steer what we pick next.

The editor and the GUI framework are the mature part of Orbit: atlas is
~13k lines and aurora ~6k, against ~1.9k for helios and ~1.6k for photon.
`comet` and `voyager` are still one-line stubs. Concretely, that means:

- **helios has exactly one component: `Sprite`.** The Node/Component model,
  reflection, serialization, and undo all work - but there is only one kind of
  capability to attach. This is why "Add Component UI" keeps not happening:
  there is nothing to add.
- **photon draws exactly one thing: textured quads.** The editor's grid, axes,
  and gizmos are all faked with sprites (`grid_sprites`, `gizmo_sprites`).
  There are no line, circle, or arc primitives, and no world-space text.
- **There is no input abstraction in the engine**, so nothing can be played
  even in principle.

So the next few bundles have a choice to make: keep polishing the authoring
experience, or start giving the engine something worth authoring. The scene
you can edit so nicely is still, today, a pile of static sprites.

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

## Toward Comet (M4)

M4 wants a code editor with live error squiggles. Some of that is compiler
work, but these pieces are aurora/atlas work that can land beforehand - and
two of them are framework changes worth knowing about early.

- **Styled text runs.** `DrawCommand::Text` carries one color for the whole
  run, and a widget has a single `foreground`. Syntax highlighting needs
  per-span colors. This is a real widget-model change (a text widget whose
  content is a list of styled spans), not a tweak - the earlier we decide its
  shape, the better.
- **Text decorations**: squiggly/straight underlines in a given color, for
  error and warning markers. Same draw-list question as styled runs.
- **A monospace font.** Only DejaVu Sans (Regular/Bold/ExtraLight) is bundled;
  a code editor needs a fixed-pitch face, and the font stack currently forces
  one family.
- **Gutter support**: line numbers, current-line highlight, and a click target
  per line (breakpoints later). Probably a text-area capability rather than a
  separate widget.
- **Find/replace** in a text area, with match highlighting.
- **Autocomplete popup**: a list positioned at the caret with keyboard
  selection - the dropdown widget below, plus caret-relative anchoring.

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

## Aurora: missing capabilities (framework-level)

The audit retired most of the original list (scrollbar drag and track-click,
Enter-to-next-row, font weights, text-area scrolling, SDF anti-aliasing,
theming-as-data, ellipsis, drag ghost and drop highlight all shipped). What is
genuinely still missing:

- **Dropdown/select widget.** Still the biggest hole: it gates the Add
  Component UI, asset-kind pickers, and Comet's autocomplete.
- **Tooltips as a framework feature** (popup + hover timer). atlas has a
  hand-rolled file tooltip; that pattern should move into aurora so anything
  can have one.
- **Richer context menus**: submenus, separators, icons, keyboard navigation.
  The popup layer and basic item lists shipped.
- **Numeric steppers** (+/- arrows on a numeric field). The slider and the
  drag-scrub shipped; discrete nudging did not.
- **Toggle switch** as a friendlier boolean than the checkbox.
- **Tabs widget.** atlas's dock grew its own tab bar (with per-corner radii and
  per-side borders underneath it); the reusable widget never got extracted.
- **Keyboard scrolling** for scroll containers (PgUp/PgDn/Home/End) - there are
  no key variants for page movement yet.
- **DPI awareness.** Font sizes and metrics are physical-pixel constants and
  nothing reads winit's scale factor; on a hidpi display the whole editor will
  be tiny. This will bite the first time Orbit runs on someone else's laptop.
- **Icon polish**: node-type icons in the scene tree, and the file-type icons
  that are already defined but never wired into the explorer rows. Disabled
  state has no icon tint (hover does).
- **Disabled state at call sites**: `Style::disabled()` works; Save-when-clean
  and Load-when-no-project still do not use it.
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

- **Frame-budget regression tests.** `Ui::last_measure_count` now counts all
  shaping, and a narrowing-drag test guards it. The same treatment would suit
  draw-list size, widget count, and rebuild frequency: cheap headless
  assertions that fail when an interaction gets quietly expensive.
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
