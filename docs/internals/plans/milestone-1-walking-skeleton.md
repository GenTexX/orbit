# Plan - Milestone 1: Walking Skeleton

**Milestone goal.** A sandbox binary opens a window and renders many textured sprites through `orbit-renderer`'s instanced 2D pipeline at real-time framerates, with `profiling` + puffin instrumentation streaming to an external `puffin_viewer`. A headless offscreen render is pixel-compared against a committed reference image. This proves the vertical stack winit -> wgpu -> `orbit-renderer` -> profiling.

**Done when:**

- The sandbox window shows a PNG sprite, and a stress mode draws N (up to ~10k) instanced sprites in a single draw call.
- `puffin_viewer`, connected over TCP, shows per-frame spans; raising N visibly grows the frame.
- An offscreen golden-image test renders a sprite headlessly and matches its reference within tolerance.
- `cargo test`, `cargo clippy -D warnings`, and `cargo fmt --check` are green; one criterion bench is recorded.

## Decisions carried in (from the 2026-07-22 planning session)

- **Coordinate system:** Y-down, origin top-left, units = pixels. See [ADR 0012](../../adr/0012-2d-coordinate-system.md).
- **Profiler:** the `profiling` crate facade with the puffin backend (Tracy-swappable later). Deep timing is viewed in the external `puffin_viewer`; there is **no on-screen readout** in M1 - a proper in-viewport overlay waits for Aurora + text (M2+).
- **Sprite:** a real PNG uploaded as a texture and drawn as an instanced quad, stress-testable to N.
- **Windowing boundary:** the sandbox binary owns winit and the event loop; `orbit-renderer` is windowing-agnostic - it takes a `raw-window-handle` target plus a size, so the exact same render path also runs headless (offscreen) for tests.

## Scope

**In:** winit window + event loop; wgpu init (instance, surface, adapter, device, queue, config, resize, sRGB surface format); `orbit-renderer` with a 2D `Camera`, `Texture` (from raw RGBA bytes), an instanced sprite pipeline, and a WGSL sprite shader; PNG decode in the sandbox; N-sprite stress mode; `profiling`+puffin+`puffin_http`; offscreen render-to-texture + readback; math unit tests; one criterion bench; `tracing` logging.

**Out (and which milestone owns it):** any GUI / Aurora, text rendering, on-screen profiler overlay -> M2. Scene tree / nodes / components, real input handling -> M3. The `shapes` / `lines` / `text` draw APIs -> later (the sprite API is *shaped* to extend to them, but they are not built). Audio, physics, asset import pipeline, multiple windows -> post-spine.

## Crate & dependency layout

**`orbit-renderer`** (owns all wgpu; ADR 0001):
- deps: `wgpu`, `glam` (math), `bytemuck` (POD buffer casts), `profiling` (facade), `thiserror` (library error type), `raw-window-handle` (surface target trait).
- `Renderer::new(...)` is **synchronous** and blocks internally with `pollster` - async wgpu setup never leaks to callers.
- `Texture` is built from raw RGBA8 bytes + dimensions; image *decoding* stays out of the renderer.
- dev-deps: `image` (decode the reference PNG in the golden-image test), `pollster`.

**`crates/sandbox`** (new, un-shipped dev playground that will grow across milestones):
- deps: `winit`, `image` (PNG decode), `orbit-renderer`, `profiling` + `puffin` + `puffin_http`, `tracing` + `tracing-subscriber`, `anyhow` (binary error handling), `glam`.

**Workspace polish:** `[profile.dev.package."*"] opt-level = 3` so wgpu/dependencies stay fast in debug builds while our own crates keep debuginfo.

## Ordered steps (each ~ one commit)

1. **Headless renderer core (TDD anchor).** In `orbit-renderer`: create instance/adapter/device/queue with no surface, and the `Camera` with its Y-down/top-left/pixels projection. Unit-test the projection and world->clip math. *Deliverable:* math tests pass; device inits headless.
2. **Offscreen sprite + golden image.** Add `Texture`, the sprite instance struct, the WGSL shader, and the instanced pipeline; render one sprite to an offscreen target and read the pixels back. Commit a reference PNG and add a tolerance-based golden-image test. *Deliverable:* the full render path is proven with no window.
3. **Windowed presentation.** In `sandbox`: open a winit window, hand it to `orbit-renderer` to create a surface, render each frame, handle resize. Present mode `Fifo` (vsync) by default with a toggle to `Immediate` for uncapped frame-time measurement. Load a real PNG and draw one sprite. *Deliverable:* a sprite in a window.
4. **Instanced stress mode.** Draw N sprites (N from env/CLI), lightly animated so the scene is alive, in a single instanced draw call. *Deliverable:* ~10k sprites on screen.
5. **Profiling wiring.** Add the `profiling` facade with the puffin backend and a `puffin_http` server; wrap frame phases (build-batch, encode, submit, present) in `profiling::scope!` and mark frame boundaries. *Deliverable:* connect `puffin_viewer`, see spans, watch the frame grow with N.
6. **Bench, CI, polish.** A criterion bench for instance-buffer building at N sprites (first entry on the benchmarks page); wire the golden-image test into CI (see below); `tracing-subscriber` logging; a short renderer page under Internals. *Deliverable:* green CI + recorded bench + docs.

## Testing strategy

- **Unit (always in CI, deterministic):** camera projection, world->clip transform, sprite-instance byte packing.
- **Golden-image (GPU):** offscreen render vs committed reference PNG, with a small per-pixel tolerance for driver variance.
- **Bench (criterion):** instance-buffer build for N sprites - the baseline we compare future frames against.
- **Manual demo:** windowed sandbox + `puffin_viewer`.

**CI GPU caveat.** GitHub Actions runners have no GPU. Plan: install Mesa **lavapipe** (software Vulkan) so the golden-image test runs headless in CI, as wgpu's own CI does. If lavapipe proves flaky, mark that single test `#[ignore]` on CI (it always runs locally on a real GPU) - the deterministic unit tests still gate every push regardless.
