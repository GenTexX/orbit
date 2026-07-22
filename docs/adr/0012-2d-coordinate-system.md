# 2D coordinate system: Y-down, origin top-left, units are pixels

World space is Y-down with the origin at the top-left: `(0, 0)` is the top-left of the camera's view, `+X` goes right, `+Y` goes down, and one world unit is one pixel. Chosen over Y-up because it matches screen space, matches the mouse coordinates the editor already receives, matches wgpu's top-left texture UV origin (so sprites are not flipped), and matches Godot - the editor's stated inspiration and the users' likely muscle memory.

## Consequences

- The camera's projection maps world pixels to wgpu's NDC (which is Y-up, `-1..1`) by flipping Y: `x_ndc = (x / w) * 2 - 1`, `y_ndc = 1 - (y / h) * 2`. The Y-flip lives in exactly one place - the projection matrix - and nowhere else.
- Physics and math code that instinctively assumes `+Y` is up must account for the flip. This is the accepted cost of screen-aligned world space.
