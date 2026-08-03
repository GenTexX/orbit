# Scripting reference

Generated from the tables in the source - see `crates/atlas/src/manual.rs`.
Editing this file by hand will be undone by the next test run.

## What the engine calls

A script is a `.cmt` file attached to a node through a Script component. The engine looks for these three functions by name; a script needs none of them, and a misspelled one is a warning rather than silence.

- `func update(dt: f32)`
- `func start()`
- `func on_destroy()`

## What a script can reach

The host surface, which the engine supplies rather than the language defining (ADR 0020). Read-only properties are the engine telling the script something; assigning to one is a compile error rather than a silent no-op.

| Property | Type | |
|---|---|---|
| `transform.position` | `Vec2` |  |
| `transform.rotation` | `f32` |  |
| `transform.scale` | `Vec2` |  |
| `input.left` | `bool` | read-only |
| `input.right` | `bool` | read-only |
| `input.up` | `bool` | read-only |
| `input.down` | `bool` | read-only |
| `input.jump` | `bool` | read-only |
| `input.mouse` | `Vec2` | read-only |
| `time.elapsed` | `f32` | read-only |
| `time.frame` | `f32` | read-only |

## Builtin functions

- `func print(s: String)`
- `func vec2(x: f32, y: f32) -> Vec2`
- `func abs(a: f32) -> f32`
- `func sqrt(a: f32) -> f32`
- `func floor(a: f32) -> f32`
- `func ceil(a: f32) -> f32`
- `func min(a: f32, b: f32) -> f32`
- `func max(a: f32, b: f32) -> f32`
- `func str(value: f32) -> String`
- `func int(value: f32) -> int`
- `func len(a: Array<T>) -> int`
- `func push(a: Array<T>, value: T)`
- `func copy(a: Array<T>) -> Array<T>`
- `func sin(a: f32) -> f32`
- `func cos(a: f32) -> f32`
- `func atan2(y: f32, x: f32) -> f32`
- `func pow(a: f32, b: f32) -> f32`
- `func random() -> f32`
- `func get(a: Array<T>, index: int) -> Option<T>`

## Keys a game reads

Held-key state, not events: `input.left` is true for as long as the key is down. Physical positions rather than letters, so the shape under three fingers is the same on every keyboard layout.

- `input.left` - ArrowLeft or KeyA
- `input.right` - ArrowRight or KeyD
- `input.up` - ArrowUp or KeyW
- `input.down` - ArrowDown or KeyS
- `input.jump` - Space

## Editor shortcuts

F1 shows this list in the editor.

### Anywhere

| Keys | What |
|---|---|
| `F1` | This list |
| `F5` | Play, or stop |
| `shift+F5` | Restart the game |
| `F11` | Maximize the pane under the pointer |
| `ctrl+S` | Save (the script, with the Code pane focused) |
| `ctrl+O` | Reload the project from disk |
| `F2` | Rename the selected node or file |
| `Delete` | Delete the selection |

### Scene

| Keys | What |
|---|---|
| `Q / W / E / R` | Select, Move, Rotate, Scale |
| `ctrl+shift+N` | Add a sprite node |
| `ctrl+shift+M` | Add a script node |
| `ctrl+shift+K` | Add a camera node |
| `ctrl+D` | Duplicate the selection |
| `ctrl+C / ctrl+X / ctrl+V` | Copy, cut, paste nodes |
| `ctrl+A` | Select every node |
| `ctrl+Z / ctrl+Y` | Undo, redo |

### Code pane

| Keys | What |
|---|---|
| `ctrl+F` | Find (and replace) |
| `F3 / shift+F3` | Next, previous match |
| `F8 / shift+F8` | Next, previous problem |
| `F12` | Go to what declares the name at the caret |
| `ctrl+click` | The same, on the name under the pointer |
| `F2` | Rename the name at the caret |
| `ctrl+G` | Go to line |
| `ctrl+shift+O` | Go to a declaration by name |
| `ctrl+M` | Jump to the matching bracket |
| `ctrl+space` | Ask for completions |
| `ctrl+/` | Comment or uncomment the selected lines |
| `ctrl+D` | Duplicate the selected lines |
| `ctrl+K` | Delete the selected lines |
| `alt+up / alt+down` | Move the selected lines |
| `ctrl+up / ctrl+down` | Previous, next declaration |
| `ctrl+shift+J` | Join the selected lines |
| `ctrl+T` | Transpose the characters around the caret |
| `ctrl+shift+U / L / Y` | Upper, lower, title case |
| `ctrl+shift+S` | Sort the selected lines (alt: drop duplicates) |
| `ctrl+Z / ctrl+Y` | Undo, redo - the script's own history |

### Files pane

| Keys | What |
|---|---|
| `ctrl+C / ctrl+X / ctrl+V` | Copy, cut, paste files |
| `ctrl+A` | Select every file |
