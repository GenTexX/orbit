# Roadmap

This is the milestone spine for Orbit. A **milestone** is a big, demonstrable capability - something you can run and show. It is *not* "a layer is finished." If it can't be demoed, it isn't a milestone. A **plan** is the ordered set of steps that gets from one milestone to the next; plans live in their own documents as each milestone is started.

**Step 0 - Scaffolding (done).** Git repository, cargo workspace of seven crates, CI (fmt/clippy/test), mkdocs site. The ground the milestones are built on.

## The spine

### Milestone 1 - Walking Skeleton (done)
**Done when:** a window opens, the 2D renderer draws many textured sprites via instanced batching, and per-frame timing streams to an external puffin viewer (no on-screen overlay yet - that waits for Aurora + text). A headless offscreen render is pixel-matched against a committed reference image.
**Result:** delivered as `photon` (instanced sprite renderer, headless pixel-tested), the `sandbox` binary (an animated N-sprite field), opt-in `tracing` logging and puffin profiling, and CI. The offscreen tests assert pixels directly rather than against a committed image (see the plan).
**Proves:** the whole vertical stack - winit -> wgpu -> `photon` -> profiling - and makes "profiling from the beginning" real instead of aspirational.
**Brings online:** `photon` (2D drawing API over wgpu), a sandbox binary, the `profiling` + puffin harness.
**Plan:** [Milestone 1 - Walking Skeleton](plans/milestone-1-walking-skeleton.md).

### Milestone 2 - Aurora (usable GUI) (done)
**Done when:** a mock inspector runs - a docked panel of labeled rows you can click and type into, laid out by taffy, holding 60fps with the profiler open.
**Proves:** the least-proven design we committed to (arena + events + handles + taffy-sync). This is deliberately early to retire that risk before the editor is built on top of it.
**Brings online:** `aurora`, `aurora-wgpu`.
**Plan:** [Milestone 2 - Aurora](plans/milestone-2-aurora.md).
**Shipped:** retained widget arena over a taffy tree; a high-level draw list rendered by aurora-wgpu through a single atlas-textured quad pipeline; text via cosmic-text (bundled DejaVu Sans) with a glyph atlas; pointer input with clip-aware hit-testing and bubbling events; buttons, checkboxes, and focusable/editable text inputs; the `inspector` demo runs well under the 16.6ms/frame budget with puffin scopes throughout. 20 headless tests.

### Milestone 3 - Scenes & Editor Shell (done)
**Done when:** the editor looks like an editor - docked scene-tree, inspector, and viewport - and you can place, move, and select sprite nodes, then save and load the Project to disk.
**Proves:** the Node + Component model, scene serialization round-trips, and Aurora assembled into a real docked application. No scripting yet.
**Brings online:** `helios` (scene tree, components, serialization, input), `atlas` (shell, viewport, scene-tree and inspector panels, file explorer).
**Plan:** [Milestone 3 - Scenes & Editor Shell](plans/milestone-3-scenes-editor.md) (ADRs 0016-0019).
**Shipped:** helios (Scene/Node/Component model with one Reflect contract, RON round-trip, undoable edit history, scene-to-photon rendering); the shared `aether` GPU crate; the atlas editor - docked resizable panels (scene-tree, viewport, inspector, file explorer), GPU picking, the full move/rotate/scale gizmo pivoting about centered sprite origins (ADR 0019), pan/zoom, undo/redo, Add Sprite + drag-a-PNG-into-the-viewport, Save/Load, toolbar, status bar, profiling.

**Iteration phase (done, 103 commits).** After the milestone the ideatank drove an open-ended polish phase, documented in six reports (3.1-3.6) written the same way as the Milestone 1 and 2 reports. In order: **3.1** aurora grew from a proof into a toolkit - scroll containers, the popup layer, text selection and clipboard, text areas, ellipsis, input masks, focus traversal, drag-and-drop, sliders, disabled state. **3.2** the editor learned to edit - insert-at-index and grouped/nested undo, tree collapse and reparent, multi-select, rename/delete/duplicate/copy-paste, search, hideable nodes, gizmo modes, the coverage-predicate icon rasterizer, a colour picker. **3.3** it became a tool you can point at a project - the data-driven dock, file explorer and operations, thumbnails, modal dialogs, asset fields, per-project editor state, the console pane, grid/axes/snapping. **3.4** appearance became data - the dark redesign, per-corner radii and per-side borders, a theme document of variables and tokens with hot reload, `prism` as a second aurora application, and the `spectrum` crate extracted once two apps shared a model. **3.5** performance - a frame profiler and a re-shape counter, the text shape cache, in-place updates, the discovery that workspace crates were compiled unoptimized, in-place texture refills, and per-frame instead of per-event input handling. **3.6** the extraction - aurora took ownership of the chrome palette, the colour picker, and a tab bar with drag-to-reorder and tear-out, plus `Style::translate`, its first animation primitive.

Two structural lessons came out of it and are worth carrying into M4. **A rebuild destroys retained interaction state**: atlas rebuilds its whole `Ui` on any change, which silently kills a live drag - found three times (a tooltip, a splitter, a tab) before the general answer landed, which is that a gesture must preview with draw-time offsets and commit only on release. And **aurora grew almost entirely by composition**: across 103 commits exactly one `WidgetKind` variant was added, while `Style` went from 4 fields to 25.

### Milestone 4 - Comet (language runs) (done)
**Done when:** you write a `.cmt` script, it compiles in milliseconds, runs on wasmtime, and moves a node - and the code editor shows live error squiggles as you type.
**Proves:** the fast-compile pipeline (lex -> parse -> check -> emit WASM, no optimizer), the wasmtime host, refcounted linear-memory objects, and the in-process language service.
**Brings online:** `comet` (frontend, WASM emission, language service), the script host in `helios`, the code editor in `atlas`.
**Plan:** [Milestone 4 - Comet](plans/milestone-4-comet.md) (ADRs 0006, 0007, 0010, 0016).
**Shipped:** `comet` end to end - an error-tolerant lexer and recursive-descent parser, a type checker emitting a typed IR, single-pass WASM emission via `wasm-encoder` with a refcounted free-list allocator in linear memory, and the in-process language service (diagnostics, syntax classification, completions). `helios::script` - the `Script` component and a wasmtime host binding a module's imports to a live `Node`'s `Transform`. Six aurora capabilities the code editor needed: a bundled monospace face, per-token colored text runs, decorations (squiggles and match highlights) built out of the mechanism selection already used, a line-number gutter that owns its own alignment, a keyboard-driven list popup, and find/replace. In atlas: `Pane::Code` with live squiggles, syntax color, autocomplete, find/replace, word-granular text undo, and drag-a-script-onto-a-node.

**Two things worth remembering.** The compiler was proven by *execution*, not by structure: the type checker proves a script is consistent and wasmparser proves the bytes are a well-formed module, but neither can tell whether `a < b` emitted `f32.lt` or `f32.gt`, whether `&&` really skips its right operand, or whether a release ever reaches the free list. Nineteen tests on a real wasmtime engine against a fake host answer those, and several of them were written to fail if an operand were swapped. And the editor's live feedback costs no rebuild at all: syntax spans and decorations are read when the draw list is built rather than when text is shaped, so re-highlighting on every keystroke regroups glyphs that are already laid out - which is what keeps the caret where it was.

A survey the day after the pane first worked found 229 things it still cannot do, and twenty of them are defects rather than gaps. That list is [code-editor-backlog.md](code-editor-backlog.md); three of its entries were reproduced with a test, and one - Tab replacing a multi-line selection with four spaces - was a same-day regression fixed on the spot.

### Iteration phase 4.1 - 4.9 - completing the language (done)

After Milestone 4 the [Comet language design record](comet-language-design.md) took fourteen decisions about what the language should become, left two questions open, and listed a further set not yet put. **Plan:** [completing the Comet language](plans/comet-language-completion.md), nine iterations ordered by dependency rather than by the order the decisions were taken.

**It runs before Milestone 5, deliberately.** That delays the killer feature, and buys building M5's hot-reload field migration once against a settled language. The connection is not incidental: M5's promise to preserve reflected field values is a sentence with no content for scripts until `@export` exists, because a `Script` component's reflected fields *are* its exported variables - today it has only `source: String`. Iteration 4.2 likewise gives input somewhere to live that is not another magic identifier. Everything from 4.5 onward (sum types, generics, containers) is language depth M5 does not need, and is where the phase could have been cut short if Play had started to matter more than completeness.

**Shipped:** all fourteen decisions of the design record plus containers and annotations, as ADRs 0020-0024. Vec2 arithmetic and the `start`/`on_destroy` hooks (4.1); the host surface as a schema, with `pos` deleted (4.2); `int` and one-way widening (4.3); `@export` and the inspector owning the value (4.4); enums with payloads, exhaustive `match` as an expression, generics monomorphized at check time, and `Option` as an ordinary prelude enum (4.5); value structs and reference arrays with `get`, `copy` and an element-walking release (4.6); the seven annotations (4.7); `const` and two warnings (4.8). 4.9, a panel showing the typed IR, was cut as that section allowed.

**Two things that kept recurring, worth carrying into M5.** A tree walk that does not handle a new node kind fails at *emission* with no source location - `collect_literals` caused exactly that three times, twice in the same function, and only a demo script using the combination ever found it. And a test can look right and check nothing: the refcount tests passed with the whole fix disabled because they used string *literals*, which are immortal, and the leak oracle was too coarse to see a 24-byte leak inside a 64KB page. Both were caught by deliberately breaking the fix and expecting red, which is now the habit rather than the exception.

### Milestone 5 - Play & Hot Reload
**Done when:** you attach a Comet script to a node, press Play, the game runs *in the viewport*, and editing the script hot-reloads it live while preserving reflected field values.
**Proves:** the killer feature and the reason the whole architecture is shaped this way - in-process runtime, reflection driving both inspector and hot-reload migration, input feeding the running game.
**Brings online:** `voyager` as a library embedded by the editor; the hot-reload field-migration path.

### Milestone 6 - Build & Ship
**Done when:** you export a Game Package and the standalone `voyager` binary plays it with no editor present.
**Proves:** Orbit is a real engine - someone can build a game and ship it.
**Brings online:** the Game Package format, the build/export command, the thin runtime binary as a shipping target.

## After the spine - breadth (M7+)

Deliberately deferred, because none of them block the spine and each is its own big thing: **physics** (rapier2d), **audio** (kira), **animation**, the **asset import pipeline**, and **instancing / prefab** polish. They become their own milestones once the spine exists.

## Open choices, revisited later

- **North-star game.** We chose not to fix a concrete demo game (Pong, a micro-platformer, ...) yet. Revisit at Milestone 5, when "what exactly are we pressing Play on?" starts to matter - a concrete target then becomes a useful scope filter.
