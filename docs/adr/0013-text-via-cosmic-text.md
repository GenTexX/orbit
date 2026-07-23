# Aurora renders text with cosmic-text

Aurora shapes, lays out, measures, and edits text with cosmic-text (the stack behind iced and Zed): it owns font loading, shaping, line layout, size measurement, and single-line editing (cosmic-text's Editor provides cursor and selection). aurora-wgpu rasterizes the positioned glyphs into a glyph atlas via cosmic-text's swash cache and batches them as textured quads.

We rejected hand-rolling glyph layout on ab_glyph or fontdue: text shaping (complex scripts, font fallback, bidi, editing) is a deep and solved problem, and a Latin-only hand-rolled layer would be thrown away the moment real text is needed - the same "don't build what we will certainly rewrite" rule that shaped the sprite instance format in Milestone 1. The GPU side (glyph atlas, quad batching, scissoring) is still ours to build and learn.

## Consequences

- The draw list carries cosmic-text glyph positions, so it is coupled to cosmic-text's text model. A non-cosmic-text draw-list backend would need a translation layer. This is accepted: text positioning is inherently tied to the text engine.
- Both aurora (shaping, layout, measurement, editing) and aurora-wgpu (rasterization into the atlas) depend on cosmic-text.
