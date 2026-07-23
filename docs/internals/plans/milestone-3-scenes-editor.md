# Plan - Milestone 3: Scenes & Editor Shell

**Milestone goal.** `atlas` opens as a real editor - a scene-tree, a live viewport, an inspector, and a file explorer in resizable, splitter-based panels - where you build a scene of sprite nodes and edit it directly: add sprites, select one by clicking it in the viewport, move it by dragging, transform it with on-screen gizmos, edit its fields in the inspector, undo and redo any of it, then save the Project to disk and load it back unchanged. This proves the three things the whole editor rests on: the Node + Component model, a reflection-driven inspector and serializer that round-trip, and Aurora (proven in M2) assembled into a real docked application driving a live engine viewport. No scripting yet.

**Done when:**

- `atlas` opens a window with resizable, splitter-based panels: scene-tree, viewport, inspector, and file explorer.
- You can add a Sprite node two ways - an "Add Sprite" action and dragging a PNG from the file explorer into the viewport - and it appears in both the scene-tree and the viewport.
- You can select a sprite by clicking it in the viewport (GPU id picking); it highlights in the viewport and the scene-tree, and its components and fields appear in the inspector.
- You can move it by dragging in the viewport, rotate and scale it with on-screen gizmos, and edit its transform and sprite fields in the inspector. Every edit is undoable and redoable.
- You can save the Project to a directory (`orbit.toml` plus a `.ron` scene) and load it back, restoring the scene exactly (a structural round-trip).
- `helios` has headless tests for the scene model, reflection, the RON round-trip, the edit-command/undo layer, and the scene-to-sprites build; `cargo test`, `clippy -D warnings`, and `fmt --check` are green.

## Decisions carried in

- **Node tree with components** (ADR 0003): a Scene is a tree of plain Nodes (name, transform, children); all capability is a Component attached to a Node.
- **Component representation and reflection** (ADR 0016): `Component` is a closed enum; each component implements a small `Reflect` trait; the inspector, serializer, and (later) hot-reload all walk that one contract.
- **Scenes serialize to RON** (ADR 0017), driven by the same reflection; the manifest is `orbit.toml` (ADR 0009).
- **A Project is a directory of text files** (ADR 0009); **scene instancing is value-overrides-only** (ADR 0011, deferred here).
- **Editor GPU integration** (ADR 0018): one shared wgpu device; photon renders the scene into a sampleable texture; Aurora composites it via a new `Image` command; selection is GPU id picking.
- **2D coordinates** (ADR 0012): Y-down, top-left, pixels; a Node's transform composes into the same 2D affine photon's `Sprite` already consumes.
- **Retained GUI** from M2 (Aurora / aurora-wgpu) and the **sprite renderer** from M1 (photon) are the substrate the editor is assembled from.
- **New this milestone:** hierarchical transforms (a child inherits its parent's transform, world = parent x local); an edit-command plus history layer for undo/redo; a resizable-splitter dock layout built as the foundation full docking grows into later.

## The editor frame cycle

The editor loop is M2's frame cycle with an engine viewport spliced in:

1. **Input.** `atlas` takes winit events and routes each to Aurora (when the pointer is over a UI panel) or to the viewport (when over the scene: pick, drag, gizmo, or pan/zoom).
2. **Edit.** Viewport and inspector interactions become edit `Command`s pushed onto the history stack; so do committed Aurora events (an "Add Sprite" click, a field edit). Undo/redo pop and re-apply from that stack.
3. **Model.** Commands mutate the `Scene` in `helios`.
4. **Render scene.** `helios` walks the Scene, composes world transforms, and emits photon `Sprite`s; photon renders them into the viewport texture (and, when a pick is pending, the id texture) on the shared device.
5. **Build UI.** `atlas` builds the Aurora tree: the scene-tree from the node hierarchy, the inspector from the selection's reflected fields, the viewport widget referencing the scene texture, and gizmo/selection overlays. Aurora lays out and emits its draw list.
6. **Composite.** aurora-wgpu renders the UI, sampling the scene texture into the viewport rect via the `Image` command, and presents.

Steps 1-4 are model logic - routing, commands, the scene, and scene-to-sprites - and are unit- and pixel-testable with no window, exactly as M1 and M2 kept their cores headless.

## Scope

**In:** `helios` (Scene / Node / Component / Transform, the `Reflect` trait and `Value` set, RON serialize/deserialize, the edit-command and undo/redo history, and scene-to-photon rendering); a small shared GPU-context crate; photon gains render-to-target and an id pass for picking; aurora gains an `Image` draw command (a texture referenced by opaque handle) and aurora-wgpu the image pipeline that resolves and samples it; `atlas` (the window, the shared device bootstrap, the docked splitter panels - scene-tree, viewport, inspector, file explorer -, selection, drag-to-translate, rotate/scale gizmos, "Add Sprite", drag-drop from the file explorer, an editor pan/zoom camera, and Project save/load). The only Component is `Sprite`. A small demo Project.

**Out (and which milestone owns it):** scripting and the `Script` component -> M4; embedding the Runtime for Play -> M5; the `Camera` component (what the game sees, distinct from the editor's own viewport camera) -> M5; scene instancing (ADR 0011) -> later; multiple open scenes and full dockable/tabbed/floating panels -> later (the splitter layout is built toward them); an asset-import pipeline -> M7+ (M3 loads PNGs directly); physics and audio -> M7+.

## Crate and dependency layout

- **A small shared GPU crate** (name TBD - Philip's call; placeholder `aether`): owns the wgpu instance/adapter/device/queue bootstrap and the validation policy currently living in photon's `gpu.rs`. Depends on `wgpu` and `pollster`. photon, aurora-wgpu, and atlas all depend on it.
- **`helios`** (the Engine): a GPU-free `scene` core (Node/Scene arena via `slotmap`, Transform via `glam`, the `Component` enum, `Reflect`/`Value`, the edit commands and history, RON via the `ron` crate with the field walk driven by `Reflect`) plus a thin `render` module that turns a Scene into photon `Sprite`s. Depends on `glam`, `slotmap`, `ron`, and `photon`. Most of helios is testable without a GPU.
- **`atlas`** (the Editor): depends on `aurora`, `aurora-wgpu`, `helios`, `photon`, the shared GPU crate, `winit`, `glam`, and `ron`. Owns the window, creates the shared device, and drives the editor frame cycle.
- **photon** and **aurora-wgpu** are refactored to accept an externally supplied device and queue (new `with_device`-style constructors) in addition to their current self-owned paths, and neither depends on the other.

## Ordered steps (each ~ one commit)

1. **helios scene model (headless).** The Node/Scene arena (a `slotmap` of `NodeId`), `Transform` (position, rotation, scale) with world-transform composition down the tree, the `Component` enum (just `Sprite`), and the `Reflect` trait plus the `Value` enum. Unit tests build a tree, compose transforms, and enumerate a component's reflected fields. *Deliverable:* a scene you can build and reflect, no GPU - the headless-first anchor.
2. **RON round-trip (headless).** Serialize a Scene to RON and read it back through the `Reflect` contract; define the Project directory (an `orbit.toml` manifest plus a scene `.ron`). Round-trip tests assert a built scene survives save-then-load structurally. *Deliverable:* scenes persist to disk and reload.
3. **Edit commands and undo (headless).** A `Command` trait (apply/revert) and a history stack (undo/redo), with the core edits expressed as commands: add, delete, and reparent a node; set a reflected field; set a transform. Tests drive a command sequence and assert undo/redo restore prior state. *Deliverable:* an undoable editing core, still no UI.
4. **Scene to photon (headless).** `helios::render` walks the Scene, composes world transforms, and emits photon `Sprite` instances; render headless and pixel-test that a sprite at a known transform lands where expected. *Deliverable:* a scene becomes pixels.
5. **Shared device and the viewport texture.** Extract the shared GPU crate; give photon and aurora-wgpu constructors that borrow an external device and queue. Add a photon render-to-target path that renders the scene into a texture created with `TEXTURE_BINDING` so it is sampleable (unlike `render_to_image`'s `COPY_SRC` readback target), exposed through new public API that hands out its `TextureView`. In aurora, add a `DrawCommand::Image { rect, texture }` carrying an opaque texture handle (no wgpu type); in aurora-wgpu, add an RGBA image pipeline plus a small external-texture registry that resolves the handle - separate from the R8 glyph-atlas path - and reconcile color space (the scene target and the surface must agree on sRGB, or the image shader converts). A bare `atlas` window shows the live scene texture in a viewport rectangle. *Deliverable:* the engine's scene, composited inside an Aurora window.
6. **Editor shell: the docked panels.** The `atlas` Aurora UI - a resizable-splitter dock layout (the foundation for full docking) holding the scene-tree (reflect the node hierarchy; select a node), the inspector (reflect the selected node's components into editable rows that emit set-field commands), the file explorer (list the Project's PNGs and scenes), and the viewport widget. Selection state is shared across panels. *Deliverable:* the editor looks like an editor.
7. **Viewport interaction: pick, move, gizmos.** GPU id picking (photon's id pass plus a one-pixel readback under the cursor) to select by clicking; drag-to-translate as a command; on-screen rotate and scale gizmos; the editor pan/zoom camera; a selection highlight. Every viewport edit goes through the command/undo history. *Deliverable:* select, move, and transform sprites in the viewport, all undoable.
8. **Placement, drag-drop, save/load, polish.** The "Add Sprite" action; dragging a PNG from the file explorer into the viewport to create a sprite there (a basic drag-and-drop); Save and Load Project wired to the menu; `profiling::scope!` across the editor frame; and the end-to-end demo Project. *Deliverable:* the Milestone 3 target.

## Testing strategy

Headless-first, as in M1 and M2: steps 1-4 (model, serialization, commands and undo, scene-to-pixels) are unit- and pixel-tested with no window. The keystone test is the round-trip - build a scene, save it, load it, and assert the result equals the original. The GPU-dependent pieces (viewport composite, id picking) get `#[ignore]`d GPU tests in the photon style, plus manual verification and a screenshot of the running editor, since a docked application is not meaningfully asserted headlessly. `clippy -D warnings` and `fmt --check` gate every step, and profiling scopes land in the final step so the editor frame is measurable.
