# Backlog - GUI and editor improvements

Improvements noted during hands-on use that are real but deliberately not
blocking the current milestone step. The rule stays demand-driven (no "polish
Aurora" milestone): pull an item in when a step touches its area, or when the
friction gets bad enough to justify a focused pass. Sourced from Philip's
testing of the Milestone 3 editor shell (2026-07-23).

## Splitters and docking

- **Wider grab area.** The splitter bar is 4px; you have to hover it exactly.
  Give splitters a hit area larger than their visual bar (Aurora hit-testing
  currently equals the drawn rect - needs a per-widget hit inset/outset).
- **Resize cursor on hover.** Aurora should expose a cursor hint for the
  hovered widget (resize-horizontal over a vertical splitter, text-beam over a
  text input, ...) and the app maps it to the winit window cursor.
- **Full docking** (tabs, drag-to-rearrange, floating panels) remains the
  stated long-term goal the splitter layout builds toward.

## Inspector fields

Everything is currently a plain text input committed on Enter. Wanted, roughly
in order:

- **Numeric fields**: accept only numbers while typing; for `Vec2`, two inputs
  in a row labeled `x` and `y`, with axis colors used consistently across the
  whole engine (viewport gizmos too - pick the palette once). Drag-to-scrub on
  the field's label to change the value with the mouse.
- **Asset fields** (e.g. a sprite's texture): an asset-chooser popup, and/or
  drag a file from the file explorer onto the field.
- **Color fields**: a color picker.

## Scene tree

- Rows are restyled buttons and read as such; they should read as tree rows.
- **Collapse/expand** for nodes with children. Note: expanded-state is
  widget-tree state and the shell rebuilds - it must live in editor state and
  be fed back in, like `PanelSizes` (see the M3 step 6 lesson), or it will
  silently reset.

## Viewport and gizmo

- **Configurable sprite anchor/pivot.** DECIDED (ADR 0019): a sprite is
  centered on its node's origin - hands-on use showed the compensation
  approach (center-pivot gizmo over a top-left-anchored model) made position
  jump confusingly. What remains here: an optional anchor/pivot *field* on
  SpriteComponent for the cases that genuinely want a corner or custom pivot,
  with centered as its default.
- **Snapping** (grid, angle increments with a modifier held) and a scale
  readout while dragging.
- **Mirror/flip via negative scale** is currently clamped away (per-axis scale
  stops at +0.05); allowing a flip needs care in the world-affine
  decomposition (negative scale flips the decomposed angle).

## File explorer

- Currently a listing only. M3 step 8 gives it its first real function
  (drag a PNG into the viewport to create a sprite); later: click-to-select
  (wired to the asset-field chooser above), previews, and file operations.
