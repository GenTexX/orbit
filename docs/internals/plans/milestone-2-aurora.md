# Plan - Milestone 2: Aurora

**Milestone goal.** A standalone mock inspector runs: a docked panel of labeled rows (each a label plus an editable value), a button, and a checkbox, laid out by taffy, that you can click and type into, holding 60fps with the profiler open. This proves the retained GUI model - the least-proven design bet in the whole plan - in a real (if small) application shape before the editor is built on it.

**Done when:**

- The mock inspector demo opens a window and shows a panel of labeled rows plus a button and a checkbox, arranged by taffy.
- You can click the button (it reports an event), toggle the checkbox, focus a text field and type into it (cursor, backspace, selection).
- It holds 60fps with `ORBIT_PROFILE=1` connected; per-phase scopes (layout, draw-list build, render) are visible.
- Aurora's deterministic core (layout results, hit-testing, event routing) has headless unit tests; `cargo test`, `clippy -D warnings`, and `fmt --check` are green.

## Decisions carried in

- **Retained model** (ADR 0004): widgets in an arena keyed by typed handles; input becomes events that bubble into a queue; the app drains the queue each frame and mutates widgets through handles. No view-diffing, no reactive signals.
- **Layout** via taffy (ADR 0005): each widget owns one taffy node; style is a taffy `Style` (size, padding, flex direction, gap).
- **Text** via cosmic-text (ADR 0013): shaping, layout, measurement, and single-line editing.
- **Widget model** (ADR 0014): one `Widget` type in a single arena, tagged by a `WidgetKind` enum.
- **Draw list** (ADR 0015): Aurora emits high-level primitives (filled rect, text run, clip push/pop); aurora-wgpu renders them. Aurora never names a wgpu type.
- **Windowing boundary** (as in M1): the demo owns winit; aurora-wgpu is windowing-agnostic, taking a `raw-window-handle` target plus a size, exactly like photon.

## The frame cycle

Retained mode here is a loop the demo drives each frame:

1. **Input.** The demo feeds winit events (mouse move/press/release, key, char) into Aurora.
2. **Route.** Aurora hit-tests using the previous frame's layout rects and turns input into events (`Clicked(id)`, `Toggled(id, bool)`, `TextChanged(id, String)`, ...) pushed onto a queue. Being one frame stale on rects is fine for UI.
3. **Drain.** The demo drains the queue, matches on events, and mutates its own state and the widgets through handles.
4. **Layout.** If the tree or any style changed, Aurora syncs the taffy tree and runs taffy, producing a rect per widget.
5. **Build.** Aurora walks the tree and emits the draw list (rects, text runs, clips).
6. **Render.** aurora-wgpu consumes the draw list, batches quads and glyph draws, and presents.

Steps 4-6 are pure functions of the widget tree, which is why steps 4 and the routing in step 2 are unit-testable with no window.

## Scope

**In:** the `aurora` framework (widget arena, tree, taffy integration, event routing, focus, the draw-list types, cosmic-text layout and single-line editing); `aurora-wgpu` (window surface, quad-batch pipeline, glyph atlas, scissoring); the five widgets (panel, label, button, checkbox, single-line text input); a mock-inspector demo; headless tests; profiling scopes.

**Out (and which milestone owns it):** docking, multi-panel editor chrome, and embedding a game viewport -> M3 (Aurora here renders one panel, not a full editor shell). Multi-line text editing, scrolling, and a real code editor -> later. Sharing one wgpu device between photon and aurora-wgpu -> M3, when both live in the editor. Theming, animations, drag-and-drop, accessibility -> post-spine.

## Crate and dependency layout

**`aurora`** (the framework; no wgpu, no winit - reusable outside Orbit):
- deps: `taffy` (layout), `cosmic-text` (shaping, layout, measurement, editing), `slotmap` (the arena), `glam` (vectors and rects).
- Public surface: a `Ui` owning the arena and taffy tree; typed widget handles; builder-style construction (`ui.panel(...)`, `ui.label(parent, text)`, returning handles) plus imperative mutation (`ui.set_text(id, ...)`); an `input()` entry that accepts platform-agnostic input; a `draw_list()` that returns the current frame's commands.

**`aurora-wgpu`** (the wgpu backend; windowing-agnostic like photon):
- deps: `wgpu`, `bytemuck`, `glam`, `cosmic-text` (rasterization via its swash cache), `aurora` (to consume its draw-list types), `raw-window-handle`.
- Owns a surface, a rect/quad-batching pipeline, a dynamically-packed glyph atlas, and scissor-rect clipping. Its wgpu setup mirrors photon's; the shared init may be extracted into a small crate later, but for now it is duplicated so `aurora` stays independent of `photon`.

**The demo** owns winit, creates the window, builds the inspector with the `aurora` API, and each frame feeds input to `aurora` and hands its draw list to `aurora-wgpu`. Likely an example under `aurora-wgpu` or a small `crates/aurora-sandbox` binary; decided at step 2.

## Ordered steps (each ~ one commit)

1. **Aurora core: arena, tree, and taffy layout (headless).** The `Widget` arena and `WidgetKind` enum, parent/child tree, a taffy node per widget, and a layout pass that runs taffy and yields a rect per widget. Style expressed as taffy `Style`. Unit tests build a tree, run layout, and assert computed rects. *Deliverable:* a tree lays out correctly with no rendering - the headless-first TDD anchor, as in M1.
2. **Draw list plus aurora-wgpu: boxes on screen.** Define the draw-list command enum (filled rect, clip push/pop). aurora-wgpu: a surface, a quad-batching pipeline, scissor for clips. The demo renders a panel with colored child rects laid out by taffy. *Deliverable:* a laid-out box tree in a window.
3. **Text: cosmic-text plus a glyph atlas.** Shape and measure label text with cosmic-text (so a label's size feeds layout); aurora-wgpu builds a glyph atlas and renders text-run commands. *Deliverable:* labeled rows.
4. **Input and events: hit-testing, bubbling, the queue.** Feed winit input into Aurora, hit-test against taffy rects, produce bubbling events, drain them in the demo. Button (click) and Checkbox (toggle) working. *Deliverable:* a clickable button and a toggleable checkbox that report events.
5. **Text input: focus and editing.** Single-line editable field on cosmic-text's Editor: a focus model, cursor, key and char handling, selection. *Deliverable:* type into a field.
6. **Mock inspector, profiling, polish.** Assemble the docked panel of labeled editable rows plus the button and checkbox; wrap layout, draw-list build, and render in `profiling::scope!`; confirm 60fps; tests and a short Aurora internals doc. *Deliverable:* the Milestone 2 target.

## Testing strategy

- **Unit (always in CI, deterministic, no GPU):** layout results (build a tree, run taffy, assert rects), hit-testing (a point maps to the expected widget), event routing and bubbling, focus transitions, and text measurement. This is the bulk of the value and the reason the model was chosen to be testable.
- **Rendering (GPU):** golden-image or pixel-assertion tests of small draw lists offscreen, `#[ignore]`d like photon's, once output stabilizes.
- **Manual demo:** the mock inspector plus `puffin_viewer`.
