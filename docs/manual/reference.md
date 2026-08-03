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
