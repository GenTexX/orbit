# Renderer (`photon`)

`photon` is Orbit's 2D drawing layer and the **only** crate that touches wgpu types ([ADR 0001](../adr/0001-wgpu-not-raw-vulkan.md)). Everything above it - the engine, the editor's viewport - draws through a 2D API (sprites, shapes, text, render targets) and never sees wgpu. That confinement is what keeps the graphics backend replaceable: swapping it out is a rewrite of this one crate's internals, not a change at every call site.

This page grows with the renderer. Today it covers what exists after Milestone 1, Step 1.

## Coordinate system

World space is **Y-down, origin top-left, one unit = one pixel** ([ADR 0012](../adr/0012-2d-coordinate-system.md)) - matching screen space, editor mouse coordinates, and Godot. The flip from Y-down world space to wgpu's Y-up clip space lives in exactly one place: `Camera::view_projection`, which builds the orthographic matrix by hand so that flip is explicit and testable rather than hidden inside a library helper.

## Current shape

- **`Gpu`** (internal) - the shared wgpu instance, adapter, device, and queue. A single async setup path serves both cases: *headless* (no surface - used by tests and offscreen render targets) and, later, *windowed* (an adapter chosen compatible with a surface). Public constructors are **synchronous**; the async setup is blocked internally so nothing above the renderer is forced to become async.
- **`Camera`** - a 2D camera producing the world -> clip view-projection matrix, unit-tested against the corner and pan mappings ADR 0012 promises.
- **`Renderer`** - builds the sprite pipeline once, then draws batches. It renders **to a window** (`new` + `render`, which acquires and presents a surface frame; `resize` reconfigures it) and **offscreen** (`render_to_image`, used by tests and later by editor render targets). Both paths share the batch-building and pass-recording code and differ only in target format: the offscreen format is linear for exact read-back, the window uses the surface's own (sRGB) format.
- **`Sprite` / `Texture` / `Color`** - the public 2D drawing vocabulary. A `Sprite` is a textured quad placed by a 2D affine (position, size, rotation); every sprite in a call shares one `Texture`. Per-sprite data is packed into an instanced vertex buffer and expanded from a unit quad in `sprite.wgsl`. The affine format is deliberately general - it is exactly what the Milestone 3 scene graph will produce for nested, rotated, scaled nodes - so it will not need rewriting then.

The **`sandbox`** binary (`crates/sandbox`) drives the windowed renderer: it owns the winit event loop ([ADR 0002](../adr/0002-runtime-as-library-in-process-play.md)), decodes an embedded PNG into a texture, and draws a field of animated sprites - count from the `ORBIT_SPRITES` env var (default 10,000) - in a single instanced draw call, logging FPS once a second. The sample PNG is produced reproducibly by `cargo run -p sandbox --example gen_sprite`. The sprite count is a tunable load: it scales roughly linearly (10k sprites ~2.4 ms/frame, 100k ~19 ms), giving the profiler something real to measure.

## How it's tested

The renderer's correctness is pinned by GPU tests that render offscreen and assert actual pixel values - e.g. a red sprite placed near the world origin must appear in the **top-left** of the image, proving the Y-down coordinate system through the real pipeline. These assertions check *correctness* (orientation, colour, coverage), not merely frame-to-frame determinism, and need no committed reference image. They require a GPU, so they are `#[ignore]`d until lavapipe runs in CI (Step 6); the deterministic camera-math tests gate every push. A committed golden-image comparison becomes worthwhile later, once there is richer visual output (alpha blending, filtering) worth locking byte-for-byte.

## Logging, profiling, and validation

photon emits `tracing` events (e.g. the acquired adapter and backend); binaries install the subscriber. The sandbox does, and also captures `log` records from wgpu and winit, so `RUST_LOG=debug cargo run -p sandbox` surfaces everything.

Frame profiling uses the `profiling` facade over the puffin backend, so the same `profiling::scope!` instrumentation can later target Tracy instead. photon scopes the render phases (acquire, prepare_batch, record_pass, submit, present); the sandbox scopes `build_scene` and marks each frame boundary. It is **opt-in**: `ORBIT_PROFILE=1 cargo run -p sandbox` turns puffin on and serves it on `127.0.0.1:8585`; connect the standalone `puffin_viewer --url 127.0.0.1:8585` to see per-phase timing (wgpu's own scopes appear too, since it also uses `profiling`). With `ORBIT_PROFILE` unset, the scopes are cheap no-ops.

Vulkan validation layers are **off by default** and opt-in with `WGPU_VALIDATION=1`. They are a debugging aid rather than always-on noise, and wgpu 30.0.0 has a benign but very chatty validation bug on Linux: it only resets the swapchain acquire fence on Windows (`cfg(windows)` in wgpu-hal `vulkan/swapchain/native.rs`), so on Linux it reports `VUID-vkAcquireNextImageKHR-fence-10066` every frame. Rendering is correct regardless - the real acquire-to-render synchronization is via semaphores, not that fence. Turn validation on when developing GPU code; remove this note once the upstream fix ships.

## Not here yet

The criterion bench and CI wiring (Step 6, the last of Milestone 1) come next. Multiple surfaces (for the editor's viewports), a per-format pipeline cache, and a persistent instance buffer (the batch is rebuilt and re-uploaded every frame today) come with later milestones. See the [Milestone 1 plan](plans/milestone-1-walking-skeleton.md).
