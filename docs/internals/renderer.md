# Renderer (`orbit-renderer`)

`orbit-renderer` is Orbit's 2D drawing layer and the **only** crate that touches wgpu types ([ADR 0001](../adr/0001-wgpu-not-raw-vulkan.md)). Everything above it - the engine, the editor's viewport - draws through a 2D API (sprites, shapes, text, render targets) and never sees wgpu. That confinement is what keeps the graphics backend replaceable: swapping it out is a rewrite of this one crate's internals, not a change at every call site.

This page grows with the renderer. Today it covers what exists after Milestone 1, Step 1.

## Coordinate system

World space is **Y-down, origin top-left, one unit = one pixel** ([ADR 0012](../adr/0012-2d-coordinate-system.md)) - matching screen space, editor mouse coordinates, and Godot. The flip from Y-down world space to wgpu's Y-up clip space lives in exactly one place: `Camera::view_projection`, which builds the orthographic matrix by hand so that flip is explicit and testable rather than hidden inside a library helper.

## Current shape

- **`Gpu`** (internal) - the shared wgpu instance, adapter, device, and queue. A single async setup path serves both cases: *headless* (no surface - used by tests and offscreen render targets) and, later, *windowed* (an adapter chosen compatible with a surface). Public constructors are **synchronous**; the async setup is blocked internally so nothing above the renderer is forced to become async.
- **`Camera`** - a 2D camera producing the world -> clip view-projection matrix, unit-tested against the corner and pan mappings ADR 0012 promises.
- **`Renderer`** - builds the sprite pipeline once, then draws batches. Today it renders **offscreen**: `render_to_image` rasterizes a batch of sprites through a camera into an image and reads the pixels back to the CPU. The windowed path (Step 3) reuses the same pipeline.
- **`Sprite` / `Texture` / `Color`** - the public 2D drawing vocabulary. A `Sprite` is a textured quad placed by a 2D affine (position, size, rotation); every sprite in a call shares one `Texture`. Per-sprite data is packed into an instanced vertex buffer and expanded from a unit quad in `sprite.wgsl`. The affine format is deliberately general - it is exactly what the Milestone 3 scene graph will produce for nested, rotated, scaled nodes - so it will not need rewriting then.

## How it's tested

The renderer's correctness is pinned by GPU tests that render offscreen and assert actual pixel values - e.g. a red sprite placed near the world origin must appear in the **top-left** of the image, proving the Y-down coordinate system through the real pipeline. These assertions check *correctness* (orientation, colour, coverage), not merely frame-to-frame determinism, and need no committed reference image. They require a GPU, so they are `#[ignore]`d until lavapipe runs in CI (Step 6); the deterministic camera-math tests gate every push. A committed golden-image comparison becomes worthwhile later, once there is richer visual output (alpha blending, filtering) worth locking byte-for-byte.

## Not here yet

The windowed surface/present path (Step 3), the instanced stress test and animation (Step 4), and profiling instrumentation (Step 5) arrive over the rest of Milestone 1. See the [Milestone 1 plan](plans/milestone-1-walking-skeleton.md).
