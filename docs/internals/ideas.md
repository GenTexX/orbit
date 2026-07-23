# Ideatank

The idea inbox: anything worth doing that we are not doing right now lands
here, one bullet per idea, grouped by area. No commitment implied - an entry
is a captured thought, not a promise.

**The ritual:** at the end of every milestone, before it is declared done, we
go through this list together and pick what to implement as part of the
milestone's finishing pass. Everything else stays for the next round. When an
idea ships, delete its entry (git history remembers); when one dies, delete
it too.

Seeded from hands-on M3 testing and the big brainstorm session at the end of
M3 (2026-07-23), with an eye on M4-M6 shifting focus away from the editor:
this list is what "come back and make the editor great" concretely means.

## Aurora: missing capabilities (framework-level)

- **Scrolling containers.** The number one gap: the scene tree, inspector,
  and file list all overflow silently today. Needs a scroll offset per
  container, wheel routing, clip (exists), and a scrollbar widget - and
  scroll positions must live in editor state across shell rebuilds (the
  PanelSizes lesson).
- **Text selection** in inputs (shift+arrows, mouse sweep), plus
  **clipboard** (ctrl+c/x/v via winit/arboard) - editing fields without
  selection gets old fast.
- **Tab focus traversal** between inputs (and Enter moving to the next row in
  the inspector, spreadsheet-style).
- **Popup/overlay layer**: widgets drawn above the normal tree with their own
  hit-testing pass - the foundation dropdowns, context menus, tooltips,
  dialogs, the asset chooser, and the color picker all stand on. Probably the
  highest-leverage single Aurora feature.
- **Dropdown/select widget** (needs popups).
- **Context menus** on right-click (needs popups; the scene tree and viewport
  both want them badly).
- **Tooltips** on hover (needs popups + a hover timer).
- **Modal dialogs** (confirm-discard-changes, errors) - popups plus an input
  block behind them.
- **Sliders and numeric steppers** (drag a value, click tiny +/- arrows).
- **Icons.** Everything is text; a small bitmap icon atlas (same machinery as
  the glyph atlas) would transform the toolbar, tree rows (node type icons,
  visibility eyes), and file list.
- **Disabled state** for widgets (grayed out, non-interactive) - e.g. Save
  when nothing changed, Load when no project.
- **Per-side padding/margin in Style.** The tree indents with a spacer-panel
  hack today because padding is all-sides-equal.
- **Theming as data**: one Theme struct (colors, spacing, font sizes) instead
  of scattered consts, so the editor can restyle Aurora without editing it.
- **DPI awareness**: font size and metrics are physical-pixel constants; on a
  hidpi display everything will be tiny. Respect winit's scale factor.
- **Text ellipsis/truncation** for long labels (file paths, node names).
- **Cursor hints**: Aurora reports a cursor per hovered widget (resize arrows
  over splitters, text beam over inputs); the app maps it to winit.
- **Wider splitter grab area** (hit area larger than the drawn bar - needs a
  per-widget hit inset).
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
- **Status bar** along the bottom: cursor world position, zoom, selected
  node, fps - cheap and very "real editor".
- **Checkerboard or configurable background** behind the scene (communicates
  transparency; the flat navy reads as part of the scene).
- **Gizmo modes with a toolbar above the viewport** (Godot-style): select /
  move / rotate / scale as exclusive modes with Q/W/E/S-style shortcuts; only
  the active mode's gizmo shows. The toolbar now exists to host it.
- **Corner scale free (non-uniform) by default**, modifier (Shift, per
  Godot/Figma convention) to preserve ratio - inverts today's uniform-only
  corner handle.
- **Snapping**: grid snap for moves, angle increments for rotation (modifier
  held), plus a numeric readout near the cursor while dragging.
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

- **Numeric fields**: accept only numbers while typing; for `Vec2`, two
  inputs labeled `x` and `y` in the engine axis colors (X red, Y green).
  Drag-to-scrub on the field's label.
- **Rotation in degrees** (stored radians, displayed/edited as degrees).
- **Asset fields**: an asset-chooser popup (needs popups), and/or drag a file
  from the explorer onto the field.
- **Color fields**: a color picker (needs popups); until then at least a
  color swatch preview next to the text.
- **Commit on focus loss** in addition to Enter (clicking away currently
  silently keeps the old value - surprising).

## Engine (helios / photon)

- **Texture cache + per-texture batching.** THE silent limitation: every
  sprite renders with the single loaded demo texture regardless of what its
  `texture` field says. The editor needs a path-keyed texture cache and
  photon needs to draw batches grouped by texture. Invisible with one PNG in
  the project, wrong the moment there are two.
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
