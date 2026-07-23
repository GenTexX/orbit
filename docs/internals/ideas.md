# Ideatank

The idea inbox: anything worth doing that we are not doing right now lands
here, one bullet per idea, grouped by area. No commitment implied - an entry
is a captured thought, not a promise.

**The ritual:** once a milestone is basically done, an open-ended iteration
phase begins: we go through this list together, pick a bundle, implement it,
and repeat for as long as we want. Iteration work is not part of the
milestone itself. When an idea ships, delete its entry (git history
remembers); when one dies, delete it too.

Seeded from hands-on M3 testing and the big brainstorm session at the end of
M3 (2026-07-23), with an eye on M4-M6 shifting focus away from the editor:
this list is what "come back and make the editor great" concretely means.

## Aurora: missing capabilities (framework-level)

- **Scrollbar interactions**: dragging the thumb, clicking the track to
  page, and keyboard scrolling (PgUp/PgDn/Home/End) - the wheel-only
  scrolling that shipped covers the common case but not these.
- **Enter moves to the next inspector row** (spreadsheet-style), building on
  the shipped tab traversal.
- **Popup layer shipped** (right-click context menus ride on it). Still open,
  now unblocked: **dropdown/select widget**, **tooltips** on hover (popup +
  timer), **modal dialogs** (popup + input block behind), and richer context
  menus (submenus, separators, icons, keyboard nav).
- **Numeric steppers** (tiny +/- arrows on a numeric field to nudge the
  value) - the slider and the inspector drag-scrub shipped; discrete steppers
  did not.
- **Icons.** Everything is text; icons would transform the toolbar, tree rows
  (node type icons, visibility eyes), and file list. Open question on the
  format: a bitmap atlas (same machinery as the glyph atlas, simplest), SVG
  (crisp at any scale/DPI but needs a rasterizer like resvg), or a small
  custom "Aurora icon" vector format (a handful of filled/stroked paths we
  tessellate ourselves - no dependency, tuned to what we need).
- **Toggle switch** (a sliding on/off control, optionally animated) as a
  friendlier alternative to the checkbox for booleans.
- **Tabs widget**: a tab bar plus swappable content panes - the primitive the
  editor's scene tabs and any tabbed inspector would build on.
- **Text area**: a multi-line text input (wrapping, vertical scroll, caret
  navigation across lines) - distinct from today's single-line field, and the
  foundation the M4 code editor needs.
- **Multiple font sizes** (and weights): the whole UI is one size today.
  Headings, body, and small captions want distinct sizes - pairs with
  theming-as-data.
- **Input placeholders**: greyed hint text shown in an empty field (e.g.
  "search...", "name").
- **Richer input masks**: numeric masking shipped; integer-only and hex-color
  masks would round it out (the color picker will want hex).
- **Anti-aliasing** for the UI: rounded-rect and rotated-line edges are hard
  today (single-sample quads). MSAA on the aurora-wgpu pass, or SDF-based
  shapes, for crisp edges - also helps photon's gizmo overlay lines.
- **Node editor**: a pan/zoom canvas of nodes with draggable ports and wires
  between them - for a future visual scripting / shader / state-machine graph.
  A big one; needs the popup/canvas and drag-and-drop groundwork.
- **Disabled state** for widgets (grayed out, non-interactive) - e.g. Save
  when nothing changed, Load when no project.
- **Per-side padding/margin in Style.** The tree indents with a spacer-panel
  hack today because padding is all-sides-equal.
- **Theming as data**: one Theme struct (colors, spacing, font sizes) instead
  of scattered consts, so the editor can restyle Aurora without editing it.
- **DPI awareness**: font size and metrics are physical-pixel constants; on a
  hidpi display everything will be tiny. Respect winit's scale factor.
- **Text ellipsis/truncation** for long labels (file paths, node names).
- **A real checkmark** in checkboxes (undecided if wanted; the filled square
  is a legit minimalist look).
- **First-class drag-and-drop**: Aurora-level drag sources/targets with a
  drag payload and hover feedback, replacing the app-level press/release
  routing the file-to-viewport drop uses today.

## Editor: scene editing

- **Add/remove components UI.** Today a node's components are fixed at
  creation. The inspector needs "Add Component" (a dropdown of kinds) and a
  remove button per component - REQUIRED once Script (M4) and Camera (M5)
  components exist, so this one has a deadline of sorts.
- **Rename a node** (double-click its tree row, or a name field at the top of
  the inspector - Node.name is already there, just not editable).
- **Delete** the selected node (Del key + context menu), **duplicate**
  (ctrl+D), and **copy/paste** across the tree - all through History.
- **Reparent by dragging** rows in the scene tree (the drag-drop machinery
  above; History::reparent already exists and refuses cycles).
- **Multi-select**: ctrl+click in tree and viewport, box-select by dragging
  over empty viewport space; group move/delete; the inspector shows shared
  fields.
- **Sibling order = draw order controls**: move up/down in the tree (z-order
  today is implicit pre-order), maybe an explicit z-index later.
- **Node visibility toggle** (eye icon per tree row, drawn state in helios)
  and **lock** (excluded from picking).
- **Search/filter box** above the scene tree.
- **Alt-drag to duplicate** in the viewport (grab a sprite with alt held:
  duplicates it and moves the copy).

## Editor: viewport

- **Grid + origin axes**: a world-space grid (fading with zoom) and X/Y axis
  lines through the origin in the axis colors; photon needs line/shape
  primitives or tinted-quad lines for this.
- **Zoom indicator + view controls**: current zoom % readout, frame-selected
  (F), frame-all, reset view (double-click middle?).
- **Checkerboard or configurable background** behind the scene (communicates
  transparency; the flat navy reads as part of the scene).
- **Snapping**: grid snap for moves, angle increments for rotation (modifier
  held), plus a numeric readout near the cursor while dragging.
- **Arrowheads on the gizmo axes**: the move handles are plain shafts with a
  square tip today; proper arrowheads (a triangle at each end) read as a real
  transform gizmo. Needs a triangle/shape primitive in the overlay.
- **Rotate gizmo arc feedback**: while rotating, draw a partially filled
  circle/pie from the start angle to the current one, so the amount of
  rotation is visible at a glance. Needs an arc/pie primitive.
- **Mirror/flip via negative scale** is currently clamped away; allowing a
  flip needs care in the world-affine decomposition.
- **Configurable sprite anchor/pivot field** on SpriteComponent, centered by
  default (ADR 0019 settled the default; the field covers the rest).

## Editor: shell and workflow

- **Persist editor state across restarts**: panel sizes, camera pan/zoom,
  selection, window size - a small editor-state file next to the project
  (gitignored), reusing the PanelSizes capture pattern.
- **Unsaved-changes indicator** (dot/star in the title bar) and a
  confirm-on-quit prompt when dirty (needs modals).
- **New/Open project**: currently hardwired to demo_project. Needs a native
  file dialog (the rfd crate) or an own popup browser.
- **Console/log panel**: a dockable panel showing tracing output - doubly
  useful once Comet scripts print (M4).
- **Undo-history panel** (the History stack visualized, click to jump) -
  History already has the data.
- **Scene tabs / multiple open scenes** (deferred from the M3 plan; needs
  the docking work).
- **Full docking** (tabs, drag-to-rearrange, floating panels) remains the
  long-term shell goal the splitters build toward.
- **A keyboard shortcut map** (one place defining all bindings, shown in a
  help popup) before shortcuts sprawl further.

## Inspector fields

- **Asset fields**: an asset-chooser popup (needs popups), and/or drag a file
  from the explorer onto the field.
- **Color fields**: a color picker (needs popups); until then at least a
  color swatch preview next to the text.

## Engine (helios / photon)

- **Texture atlas / sort-then-batch.** Per-texture batching shipped
  (consecutive same-texture runs), but cross-texture painter's order still
  forces a new draw at every texture change. A texture atlas (one draw) or a
  z-sort-then-batch pass would cut draw calls for texture-interleaved scenes.
- **Sprite sheet support**: expose photon's `uv_rect` on SpriteComponent
  (region of a texture), the base for animation later.
- **Sprite flip flags** (flip_h/flip_v) as a cheap alternative to negative
  scale.
- **photon shape primitives**: lines, rects, circles - for the editor grid,
  debug drawing, and eventually game use.
- **World-space text** (photon reusing Aurora's glyph atlas machinery) -
  node labels in the viewport, debug overlays, in-game text later.
- **Node visibility flag** in helios (skipped by sprites() when off) -
  pairs with the tree's eye icon.

## File explorer

- Currently a listing plus drag-to-viewport. Later: click-to-select (wired to
  the asset-field chooser), thumbnails/previews for PNGs, file operations
  (rename, delete, new folder), and watching the directory for changes.
