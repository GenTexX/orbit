# Renderer (`orbit-renderer`)

`orbit-renderer` is Orbit's 2D drawing layer and the **only** crate that touches wgpu types ([ADR 0001](../adr/0001-wgpu-not-raw-vulkan.md)). Everything above it - the engine, the editor's viewport - draws through a 2D API (sprites, shapes, text, render targets) and never sees wgpu. That confinement is what keeps the graphics backend replaceable: swapping it out is a rewrite of this one crate's internals, not a change at every call site.

This page grows with the renderer. Today it covers what exists after Milestone 1, Step 1.

## Coordinate system

World space is **Y-down, origin top-left, one unit = one pixel** ([ADR 0012](../adr/0012-2d-coordinate-system.md)) - matching screen space, editor mouse coordinates, and Godot. The flip from Y-down world space to wgpu's Y-up clip space lives in exactly one place: `Camera::view_projection`, which builds the orthographic matrix by hand so that flip is explicit and testable rather than hidden inside a library helper.

## Current shape

- **`Gpu`** (internal) - the shared wgpu instance, adapter, device, and queue. A single async setup path serves both cases: *headless* (no surface - used by tests and, soon, offscreen render targets) and, later, *windowed* (an adapter chosen compatible with a surface). Public constructors are **synchronous**; the async setup is blocked internally so nothing above the renderer is forced to become async.
- **`Camera`** - a 2D camera that produces the world -> clip view-projection matrix. Its projection is unit-tested against the corner and pan mappings ADR 0012 promises.

## Not here yet

The instanced sprite pipeline, the surface/windowed present path, and offscreen render targets arrive over the rest of Milestone 1. See the [Milestone 1 plan](plans/milestone-1-walking-skeleton.md).
