# Roadmap

This is the milestone spine for Orbit. A **milestone** is a big, demonstrable capability - something you can run and show. It is *not* "a layer is finished." If it can't be demoed, it isn't a milestone. A **plan** is the ordered set of steps that gets from one milestone to the next; plans live in their own documents as each milestone is started.

**Step 0 - Scaffolding (done).** Git repository, cargo workspace of seven crates, CI (fmt/clippy/test), mkdocs site. The ground the milestones are built on.

## The spine

### Milestone 1 - Walking Skeleton
**Done when:** a window opens, the 2D renderer draws many textured sprites via instanced batching, and per-frame timing streams to an external puffin viewer (no on-screen overlay yet - that waits for Aurora + text). A headless offscreen render is pixel-matched against a committed reference image.
**Proves:** the whole vertical stack - winit -> wgpu -> `orbit-renderer` -> profiling - and makes "profiling from the beginning" real instead of aspirational.
**Brings online:** `orbit-renderer` (2D drawing API over wgpu), a sandbox binary, the `profiling` + puffin harness.
**Plan:** [Milestone 1 - Walking Skeleton](plans/milestone-1-walking-skeleton.md).

### Milestone 2 - Aurora (usable GUI)
**Done when:** a mock inspector runs - a docked panel of labeled rows you can click and type into, laid out by taffy, holding 60fps with the profiler open.
**Proves:** the least-proven design we committed to (arena + events + handles + taffy-sync). This is deliberately early to retire that risk before the editor is built on top of it.
**Brings online:** `aurora`, `aurora-wgpu`.

### Milestone 3 - Scenes & Editor Shell
**Done when:** the editor looks like an editor - docked scene-tree, inspector, and viewport - and you can place, move, and select sprite nodes, then save and load the Project to disk.
**Proves:** the Node + Component model, scene serialization round-trips, and Aurora assembled into a real docked application. No scripting yet.
**Brings online:** `orbit-engine` (scene tree, components, serialization, input), `orbit-editor` (shell, viewport, scene-tree and inspector panels, file explorer).

### Milestone 4 - Comet (language runs)
**Done when:** you write a `.cmt` script, it compiles in milliseconds, runs on wasmtime, and moves a node - and the code editor shows live error squiggles as you type.
**Proves:** the fast-compile pipeline (lex -> parse -> check -> emit WASM, no optimizer), the wasmtime host, refcounted linear-memory objects, and the in-process language service.
**Brings online:** `comet` (frontend, WASM emission, language service), the script host in `orbit-engine`, the code editor in `orbit-editor`.

### Milestone 5 - Play & Hot Reload
**Done when:** you attach a Comet script to a node, press Play, the game runs *in the viewport*, and editing the script hot-reloads it live while preserving reflected field values.
**Proves:** the killer feature and the reason the whole architecture is shaped this way - in-process runtime, reflection driving both inspector and hot-reload migration, input feeding the running game.
**Brings online:** `orbit-runtime` as a library embedded by the editor; the hot-reload field-migration path.

### Milestone 6 - Build & Ship
**Done when:** you export a Game Package and the standalone `orbit-runtime` binary plays it with no editor present.
**Proves:** Orbit is a real engine - someone can build a game and ship it.
**Brings online:** the Game Package format, the build/export command, the thin runtime binary as a shipping target.

## After the spine - breadth (M7+)

Deliberately deferred, because none of them block the spine and each is its own big thing: **physics** (rapier2d), **audio** (kira), **animation**, the **asset import pipeline**, and **instancing / prefab** polish. They become their own milestones once the spine exists.

## Open choices, revisited later

- **North-star game.** We chose not to fix a concrete demo game (Pong, a micro-platformer, ...) yet. Revisit at Milestone 5, when "what exactly are we pressing Play on?" starts to matter - a concrete target then becomes a useful scope filter.
