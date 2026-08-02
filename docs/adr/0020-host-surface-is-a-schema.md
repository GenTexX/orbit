# The host surface is a schema the engine supplies, not names the compiler knows

comet compiles against a **host schema**: named groups of typed properties, described by the engine and passed to `compile(source, &schema)`. A script reaches them by path - `transform.position`, `transform.position.x` - and the compiler contains no name, type, or accessor belonging to any of them. Adding a scriptable property is a row in a table helios owns, with no change to the lexer, parser, checker, typed IR, or codegen.

The magic identifier `pos` is gone. It is not sugar for `transform.position`; there is no `pos`.

## Why

`pos` had dedicated variants the whole length of the pipeline: `TypedExprKind::Pos`, `Place::Pos`, `Place::PosField(Axis)`, plus a magic name in the checker, three fixed host imports, and a special case in the language service. Every further property - rotation, scale, a component's fields, input - would have cost another set of the same, in every one of those places. The cost was not the first property; it was the tenth, and it grew before anything paid it back.

ADR 0002 makes the editor embed the same runtime a game does, and ADR 0016 makes the inspector, the serializer and hot-reload walk one reflection contract rather than hard-coding components. A compiler that names `Transform` is the same mistake one layer down. The schema puts comet on the same footing as those three: it consumes a description of the engine rather than containing one.

The language service is the other half. Because the schema is data, `transform.` completes to `position`, `rotation`, `scale` with their types, and hover lists what an object holds - without the service being told anything separately. A surface that is discoverable only by reading engine source is not a surface a beginner can use.

## How

**Properties are numbered, not imported.** The schema numbers every property flat across objects in declaration order, and codegen passes that number to a fixed set of accessors: `get_f32`/`set_f32`, `get_bool`/`set_bool`, `get_vec2_x`/`get_vec2_y`/`set_vec2`. The import list therefore stays fixed no matter how large the schema grows - a host that can run one comet module can run all of them, and there is one binding table to write rather than one per script. Importing a function per property would have made the module's imports depend on the schema and put the binding table's size in the schema's hands.

**`HostType` is narrower than `Type`.** It is exactly what codegen can emit an accessor for - `f32`, `bool`, `Vec2` - so a schema cannot describe something the compiler would then have to refuse. `String` is absent deliberately: a host property owning a refcounted value needs an ownership rule across the host boundary, and that is its own decision.

**Both sides derive the numbering from one list.** helios declares its properties once; the schema and the id-to-property lookup are both built from it, and a test walks the two together. Two hand-maintained orderings would drift, and the failure - every accessor reading the wrong field - would not look like a numbering bug.

**Objects are ordinary names.** `transform` is not a keyword and locals shadow it, which is why a script may still declare something called `pos`. A bare object name is an error that says what to write instead.

## Consequences

- `comet::compile` and every `comet::service` entry point take a `&HostSchema`. helios owns the engine's, atlas asks helios for it, so the editor's squiggles and completions describe exactly the surface the compiler enforces.
- Every existing script breaks. `pos` was in both demo scripts, four test fixtures and roughly forty test sources; all were rewritten. This was the cheapest this change will ever be, which is why it came before the rest of the language work rather than after.
- Component properties are **not** in the schema yet. The mechanism does not care, but "what does a script see when the node has no Sprite" is a runtime-semantics question worth answering deliberately rather than falling into. Adding them afterwards is a data change.
- `Reflect` still returns `&'static [&'static str]`. The dynamic field list (design decision 3) has no consumer until `ScriptComponent` carries exported values, so it moved to the iteration that needs it rather than being built ahead of its user.
