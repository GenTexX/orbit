# Aurora's draw list is a high-level command list, not geometry

Aurora emits a per-frame draw list of high-level primitives - filled rect, text run (positioned glyphs plus color), and clip push/pop for scissoring - not GPU geometry. aurora-wgpu turns the list into batched quads and glyph-atlas draws. This is Dear ImGui's and egui's model.

We rejected having Aurora tessellate into vertices and indices: that bakes triangles and a GPU into the framework, breaking the engine- and backend-independence that ADR 0004 requires (Aurora must not know wgpu exists). We rejected a retained display tree that the backend diffs frame to frame: needless complexity at inspector scale.

Because the list is high-level, alternative backends can consume the same output - a headless backend for tests today, a vector or SVG target later - without Aurora changing. The list is rebuilt each frame for now; dirty-tracking and caching are a later optimization, not a Milestone 2 concern.
