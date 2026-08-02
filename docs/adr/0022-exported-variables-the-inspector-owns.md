# Exported variables, and the inspector owning their values

`@export let speed: f32;` marks a script variable as inspector-editable. The value lives on the node's `Script` component, not in the source, and it is the value that runs: the script's own initializer is only the default used when the field is first created or explicitly reverted. An exported variable that carries an initializer is a **warning**, and an exported variable may carry only a type - which required making a state declaration's initializer optional.

## Why `@`

Godot's model, for Godot's reason: per-instance tuning is how two nodes run one script differently, and it is the only thing that makes a script component more than a path.

A plain `export` keyword was rejected - it burns a reserved word and still needs a second mechanism for the annotations that take arguments (`@range(0, 100)`, `@tooltip("...")`). A doc-comment convention was rejected because a typo would silently do nothing, which is the wrong failure mode for a language meant to be learned from. `@` generalizes, so the annotations that follow need no new syntax, and an unknown one is reported with a suggestion rather than ignored.

## Why an exported variable should not carry a value

Beyond the mechanism, the intended idiom: **an exported variable does not carry a meaningful value in the script**. If the inspector owns the value, a number in the source is a second answer to the same question, and the one a reader sees first is the one that loses.

That idiom is a warning rather than documentation, because a warning is the version of it a person meets at the moment it matters. It is the first time comet says something about a declaration that is otherwise perfectly legal - every existing warning is about code that does nothing, like an unused local. That is a real precedent and was taken deliberately: the language now has an opinion about idiom, and the bar for the next one should be the same, which is that following the advice is strictly better and the compiler can tell.

The warning forced a grammar change and the two shipped together. A declaration's initializer was mandatory and its type optional; that inverts. A declaration now needs **a type or an initializer**, and an exported one should carry only the type. Without this the warning would have fired on every exported variable with no way to act on it, and a warning nobody can silence is worse than no warning.

## Why this came before user-defined enums

A declaration with no initializer starts at its type's default, so every exportable type needs one. `f32` is `0.0`, `int` is `0`, `bool` is `false`, `Vec2` is the origin. A **user-defined enum has no natural default**, and that question was recorded as blocking this decision.

Scheduling `@export` before enums exist dissolves it rather than answering it. Every type in the language today has an obvious default, so the warning ships unblocked, and the enum case gets answered alongside the enum work with that work in front of us. That is the whole reason the completion plan orders these two the way it does.

## How

**The checker fills in the default.** A missing initializer becomes the type's default expression in the typed IR, so codegen never learns that an initializer can be left out. One less thing downstream has to know.

**Exported variables become exported wasm globals.** `state.speed`, and `state.home.x`/`state.home.y` for a `Vec2`. The host writes the stored values in after instantiation and **before `start`** - the initializer has already run and is only the default, and a `start` that read the default would be reading a value nobody set. The export names come from `comet::exported_globals` rather than being built by the host, so the convention has one definition.

**Only what can be stored can be exported.** `f32`, `int`, `bool`, `Vec2`. A `String` is a pointer into the module's own memory, so handing one across needs an ownership rule that is its own decision. The checker refuses the rest by name.

**A value that does not line up is dropped, not forced.** A component and a module can disagree while a source is being edited. Skipping beats writing a float into a bool.

## Reflect grows dynamic fields (design decision 3)

`Reflect::field_names` returns `Vec<String>` rather than `&'static [&'static str]`, because a `Script`'s fields are whatever its source exports. Special-casing that in the inspector and the serializer would have made ADR 0016's actual claim - one contract, three consumers, no component hardcoded - untrue.

The payoff is that an exported variable becomes an editable, saved field without the inspector or the serializer being told anything: both already walk `field_names()`. The cost is that the inspector's field plumbing assumed `'static` names throughout - `FieldRef`, `ColorTarget`, `AssetTarget` and four row vectors all held `&'static str` and were `Copy`. That assumption is exactly what this decision breaks, and unpicking it was most of the work.

## Consequences

- `ScriptComponent` is source plus stored values, and `reconcile` migrates them across an edit: a name still declared at the same type keeps its value, one that is gone goes, a new one arrives at its default. That is ADR 0008's field migration, for the one component whose fields can change without the component being touched.
- Reconciliation runs when a script's path changes, when a script is saved, and at project load - not continuously. A live file watcher, and the reload it would drive, is M5's.
- `ScriptHost::instantiate_file` takes the stored values, so every caller states what it is starting the script with.
- An exported `int` is stored and shown as an `f32`, because the inspector has no int field yet. Recorded rather than hidden.
