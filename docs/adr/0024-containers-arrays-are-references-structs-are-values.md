# Containers: arrays are references, structs are values

`struct Enemy { health: f32 }` is a value - flattened into slots, copied on assignment, never allocated. `Array<T>` is a reference - a refcounted heap object, growable, where two names mean one array. `a[i]` traps if `i` is not a position in `a`; `get(a, i)` gives an `Option<T>` instead.

The two container kinds deliberately differ, and the difference is the whole decision.

## Structs are values

ADR 0006 already said "value semantics for small structs", and this makes it true: a struct is exactly its fields, laid out one after another with no header. Nesting is offsets rather than pointers, so `o.inner.hp` reads one slot and `o.inner` is a copy. Nothing allocates, nothing is refcounted, and the release path ADR 0023 built does not grow.

The costs are real and accepted: a struct cannot contain itself, and passing a large one copies it. Reference structs were rejected because they would make structs behave unlike every other value in the language, and because they are the other place ADR 0007's cycle leak becomes reachable.

**Places generalized to make this work.** `Place::LocalField(slot, Axis)` became `Place::LocalAt { slot, offset, ty }`, so one rule covers a `Vec2` component, a struct field, and any nesting of the two. That generalization is also what an array element write needed.

## Arrays are references, and growable

A value-semantics array would have to be fixed-size: copying must be bounded and it must live in flat slots. A list of enemies that grows is not expressible that way, so arrays are references - which is also ADR 0006's "reference semantics for objects", applied to the first thing in the language that is one.

The cost is the classic Godot gotcha: `let b = a;` gives two names for one array. `copy(a)` is the escape hatch, and it is explicit precisely because the aliasing is the default.

**An array is two blocks.** A handle holding `[len, capacity, data]`, and a separate block for the elements. This is what makes growth invisible: elements inline would mean `push` moving the block, and an alias taken beforehand would still point at the old one - exactly what reference semantics promise will not happen. The handle is what is refcounted, and its release frees the element block, which is reachable through nothing else.

Elements are stored as `i32` words with floats reinterpreted, the same packing an enum payload uses, so one addressing rule covers every element type.

## Indexing traps; `get` does not

`a[i]` stays one character for the common case - a loop over an array you just built - and traps on a miss. `get(a, i)` returns `Option<T>`, which the checker makes you handle, and is what decision 8 meant when it said arrays would introduce absence. Both directions of the range are checked: a negative index is as much a miss as one past the end.

## Releasing an array walks its elements

An array may hold anything, including things that own memory. Dropping one walks its elements first - but only when the element type owns something at all, and only when the handle is about to become unowned, so an `Array<f32>` never reads its own contents to drop them.

That walk is one rule, `release_slots`, shared by an enum's payload, a struct's field and an array's element. Writing it that way is what lets an array hold any of them: `Array<String>`, `Array<Array<String>>`, and an array of an enum whose release consults a tag all go through the same path.

**Generic structs** are not built. The template machinery from ADR 0023 would carry them, but nothing needs one yet.

## Consequences

- `val_types` now depends on three tables - enums, structs, arrays - bundled as `Layouts` and threaded through codegen. Every later container inherits that.
- Frame depth has to count what a *place* needs, not only an expression: `a[i] = v` parks the handle, the address and the value. Underestimating it panics the compiler rather than emitting anything, which is at least loud.
- `collect_literals` walks places too. A string inside `a[f("x")] = v` was never interned, and emission panicked - the same failure a string inside a `match` arm caused in 4.5a, from the same cause.
