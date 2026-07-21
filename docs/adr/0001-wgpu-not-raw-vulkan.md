# wgpu as the graphics API, not raw Vulkan

Orbit is an educational 2D engine whose learning goals are engine architecture, a custom retained GUI framework, and a custom scripting language - not GPU internals. We use wgpu instead of raw Vulkan (ash) so the renderer doesn't consume the project's learning budget: raw Vulkan means months of swapchain/synchronization/allocator plumbing of which a 2D engine uses a fraction. A prior C++ engine attempt (`myengine`) stalled in exactly this swamp.

## Consequences

- Shaders are written in WGSL.
- We accept wgpu's abstraction overhead and occasional API churn.
- We do NOT build our own graphics abstraction layer (RHI) on top of wgpu. wgpu *is* the portability layer. Replaceability comes from keeping wgpu types confined to the renderer crate and exposing only a 2D drawing API (sprites, shapes, text, render targets) to the rest of the engine - swapping backends later means rewriting that one crate's internals, not touching every call site.
