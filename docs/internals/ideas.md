# Ideatank

The idea inbox: anything worth doing that we are not doing right now lands
here, one bullet per idea, grouped by area. No commitment implied - an entry
is a captured thought, not a promise.

**The ritual:** at the end of every milestone, before it is declared done, we
go through this list together and pick what to implement as part of the
milestone's finishing pass. Everything else stays for the next round. When an
idea ships, delete its entry (git history remembers); when one dies, delete
it too.

## Gizmo and viewport

- **Corner scale should be free (non-uniform) by default**, stretching both
  axes independently; holding a modifier (Shift is the Godot/Figma
  convention, Ctrl also a candidate - decide when implementing) preserves the
  aspect ratio. Today the corner handle is uniform-only.
- **Gizmo modes with a toolbar above the viewport** (Godot-style): select /
  move / rotate / scale as exclusive modes, so only the active mode's gizmo
  shows and the viewport declutters. Keyboard shortcuts to switch (Godot uses
  Q/W/E/S). The all-in-one gizmo remains the fallback until then.
- **Configurable sprite anchor/pivot field** on SpriteComponent, centered by
  default (ADR 0019 settled the default; the field covers the cases that
  genuinely want a corner or custom pivot).
- **Snapping**: grid snap for moves, angle increments for rotation (with a
  modifier held), plus a numeric readout near the cursor while dragging.
- **Mirror/flip via negative scale** is currently clamped away (per-axis
  scale stops at +0.05); allowing a flip needs care in the world-affine
  decomposition (negative scale flips the decomposed angle).

## Splitters and docking

- **Wider grab area.** The splitter bar is 4px; you have to hover it exactly.
  Give splitters a hit area larger than their visual bar (Aurora hit-testing
  currently equals the drawn rect - needs a per-widget hit inset/outset).
- **Resize cursor on hover.** Aurora should expose a cursor hint for the
  hovered widget (resize-horizontal over a vertical splitter, text-beam over
  a text input, ...) and the app maps it to the winit window cursor.
- **Full docking** (tabs, drag-to-rearrange, floating panels) remains the
  stated long-term goal the splitter layout builds toward.

## Inspector fields

- **Numeric fields**: accept only numbers while typing; for `Vec2`, two
  inputs in a row labeled `x` and `y` in the engine axis colors (X red,
  Y green - the same palette the gizmo arrows use). Drag-to-scrub on the
  field's label to change the value with the mouse.
- **Rotation in degrees**: stored radians, displayed and edited as degrees.
- **Asset fields** (e.g. a sprite's texture): an asset-chooser popup, and/or
  drag a file from the file explorer onto the field.
- **Color fields**: a color picker.

## Scene tree

- Rows are restyled buttons and read as such; they should read as tree rows.
- **Collapse/expand** for nodes with children. Note: expanded-state is
  widget-tree state and the shell rebuilds - it must live in editor state and
  be fed back in, like `PanelSizes` (the M3 step 6 lesson), or it will
  silently reset.

## File explorer

- Currently a listing only. M3 step 8 gives it its first real function
  (drag a PNG into the viewport to create a sprite); later: click-to-select
  (wired to the asset-field chooser above), previews, and file operations.
