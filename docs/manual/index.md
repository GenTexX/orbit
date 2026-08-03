# Manual

Making games with Orbit. This page is the tour; the
[scripting reference](reference.md) is generated from the source and is the
place to look a name up.

## What a project is

A directory of text files (ADR 0009): a manifest, one or more scenes, your
scripts, and your assets. All of it is meant to be read, diffed and committed -
there is no binary project format and no database.

```
orbit.toml            the manifest: a name, and which scene to open
scenes/main.ron       a scene: a tree of nodes
scripts/player.cmt    a script
assets/sprite.png     art
```

The editor opens `crates/atlas/demo_project` today. Pointing it at a directory
of your own is not built yet.

## What a scene is

A tree of **nodes**. A node has a name, a transform, and a list of
**components**; everything a node can *do* comes from a component, and a node
with none is just a position in the tree with children hanging off it.

Three component kinds exist:

- **Sprite** draws a texture at the node's transform.
- **Script** runs a `.cmt` file for the node.
- **Camera** decides what the running game sees, centred on the node it is on.

A camera being a component is the whole reason it is worth having: put one on an
empty node and the view stays put, put one on the player and the view follows,
with no code either way.

Every camera draws the rectangle it will frame, so you can aim one without
pressing Play, and a marker you can click and drag even when there is no sprite
under it. The one Play will look through is drawn bright and the rest are
dimmed.

Y grows **downward** (ADR 0012), so "below" is a greater y. A sprite is centred
on its node's origin (ADR 0019), which is what makes rotation and scale pivot
where you expect.

## Writing a script

A script is a `.cmt` file, attached to a node through a Script component. The
engine calls three functions if you write them - `start()` once before the first
frame, `update(dt: f32)` every frame, and `on_destroy()` when the node or the
game goes away. A script needs none of them.

```
// A top-level `let` is script state: it keeps its value between frames.
let velocity = vec2(120.0, 0.0);

func update(dt: f32) {
    transform.position += velocity * dt;
    if transform.position.x > 200.0 {
        velocity = -velocity;
    }
}
```

`transform.position` is this node's position - read it or assign to it. What
else a script can reach is in the [reference](reference.md); the short version
is the transform, the keyboard, and the clock.

The language is statically typed and there is no `var`: a name has one type from
its declaration onward. Numbers come in two kinds - `5` is an `int` and `5.0` is
an `f32`, and an int widens to an f32 wherever one is wanted but never the other
way without `int(x)`. Structs are values and arrays are references, deliberately
and asymmetrically, which the demo project's `shapes.cmt` and `lists.cmt` teach
at length.

### Numbers you tune rather than type

Mark a variable `@export` and the inspector owns it instead of the source:

```
@export
@range(0.0, 600.0)
@tooltip("pixels per second")
let speed: f32;
```

The value lives on the node's Script component, so two nodes running the same
file can move at different speeds. `@range` turns the field into a slider,
`@step` sets how far a drag moves it, `@tooltip` explains it, and `@readonly`
shows a value the script drives without letting you change it.

An exported variable carries no initializer - writing one would be a second
answer to the same question, and the compiler says so.

## Playing

**F5** starts the game, **F5** again stops it, **shift+F5** restarts. The
viewport is framed in orange while a game runs.

Play is a rehearsal. The scene is snapshotted when you press it and put back when
you stop, so anything the game does to the world is undone - which is why the
editor refuses scene edits while it runs, and says so in the status bar.

Two things are *not* refused, because neither is an edit to the document:

- **Selecting**, so you can aim the inspector at whatever is moving.
- **Dragging an `@export`ed value.** It goes into the running script rather than
  into the scene, so Stop puts the saved number back. This is the fastest way to
  find a jump height: play, drag until it feels right, stop, type it in for real.

**Saving a script reloads it under the running game.** Your code changes and the
game keeps going - plain state like a velocity carries across, and anything on
the heap (a String, an Array) starts fresh. `start()` deliberately does *not* run
again, so a script that places its node does not teleport it back on every save.

A script that will not compile leaves the last version that worked running and
says why. Fix it, save again, and it swaps in.

## When something goes wrong

The compiler talks to you as you type - squiggles in the Code pane, and the
Problems pane for the whole file. `squiggles.cmt` in the demo project is wrong on
purpose so you can watch it respond.

Some mistakes only show up when the game runs, and the engine has guardrails for
the four that would otherwise take the editor with them:

- A call that runs longer than 100ms is stopped: *is there a loop with no way
  out?*
- A script that asks for more than 16MB is stopped: *is something being added to
  in a loop?*
- Reading past the end of an array is stopped, and points at `len(a)` and
  `get(a, i)`.
- A function calling itself forever runs out of stack, and says so.

A stopped script is not called again, and the status bar counts how many are
running and how many have stopped. `runaway.cmt` in the demo project trips each
of these on purpose, one commented line at a time.

## The demo project

Eleven scripts, written to be read in roughly this order:

| File | What it teaches |
|---|---|
| `bounce.cmt` | state, `start`, vectors, and what a reload does |
| `tunable.cmt` | `@export` and the annotations |
| `counting.cmt` | `int` versus `f32`, `for`, and `const` |
| `shapes.cmt` | structs, which are values |
| `lists.cmt` | arrays, which are references, and `Option` |
| `states.cmt` | enums, payloads, and `match` |
| `player.cmt` | input, gravity, and a jump |
| `platform.cmt` | the clock, and why `start` exists |
| `goal.cmt` | a second script on one node |
| `squiggles.cmt` | wrong on purpose, for the compiler |
| `runaway.cmt` | wrong on purpose, for the runtime |

The scene it opens is a one-screen platformer: arrow keys or WASD to move, space
to jump, and a camera parented to the player.
